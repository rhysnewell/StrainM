use bird_tool_utils::command::finish_command_safely;
use indicatif::{style::TemplateError, MultiProgress, ProgressBar, ProgressStyle};
use itertools::Itertools;
use rayon::prelude::*;
use rust_htslib::bcf::Read;
use scoped_threadpool::Pool;
use std::cmp::min;
use std::collections::HashMap;
use std::fs::{create_dir_all, File, OpenOptions};
use std::io::{BufWriter, Write};
use std::path::Path;
use std::process::{Command, Stdio};
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tempdir::TempDir;
use tempfile::NamedTempFile;

use crate::bam_parsing::{
    FlagFilter,
    bam_generator::*
};
use crate::evolve::codon_structs::{CodonTable, Translations};
use crate::abundance::abundance_calculator_engine::AbundanceCalculatorEngine;
use crate::ani_calculator::ani_calculator::ANICalculator;
use crate::assembly::assembly_region_walker::AssemblyRegionWalker;
use crate::reference::reference_reader_utils::GenomesAndContigs;
use crate::external_command_checker::{check_for_bcftools, check_for_svim};
use crate::haplotype::haplotype_clustering_engine::HaplotypeClusteringEngine;
use crate::model::variant_context::VariantContext;
use crate::model::variant_context_utils::VariantContextUtils;
use crate::processing::bams::index_bams::*;
use crate::reference::reference_reader::ReferenceReader;
use crate::reference::reference_reader_utils::ReferenceReaderUtils;
use crate::reference::reference_writer::ReferenceWriter;
use crate::utils::errors::BirdToolError;
use crate::utils::utils::get_cleaned_sample_names;
#[cfg(feature = "fst")]
use crate::model::fst_calculator::calculate_fst;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReadType {
    Short,
    Long,
}

#[derive(Clone, Debug)]
pub struct Elem {
    pub key: String,
    pub index: usize,
    pub progress_bar: ProgressBar,
}

/// The main lorikeet engine, takes any number of reference genomes and reads/bam files and performs
/// read mapping, variant calling, consensus genome calling, and strain genotyping
///
/// @author Rhys Newell <rhys.newell@.hdr.qut.edu.au>
pub struct LorikeetEngine<'a> {
    args: &'a clap::ArgMatches,
    short_read_bam_count: usize,
    long_read_bam_count: usize,
    flag_filters: FlagFilter,
    genomes_and_contigs: GenomesAndContigs,
    concatenated_genomes: Option<NamedTempFile>,
    tmp_bam_file_cache: Option<TempDir>,
    reference_map: HashMap<usize, String>,
    references: Vec<&'a str>,
    // multi: Arc<MultiProgress>,
    multi_inner: Arc<MultiProgress>,
    tree: Arc<Mutex<Vec<&'a Elem>>>,
    progress_bars: &'a Vec<Elem>,
    threads: usize,
    mode: &'a str,
    run_in_parallel: bool,
}

impl<'a> LorikeetEngine<'a> {
    pub fn apply_per_reference(&self) {
        let parallel_genomes = *self
            .args
            .get_one::<usize>("parallel-genomes")
            .unwrap() as u32;
        let mut pool = Pool::new(parallel_genomes);
        let n_threads = std::cmp::max(
            self.threads / min(parallel_genomes as usize, self.references.len()),
            2,
        );
        let output_prefix = match self.args.contains_id("output-directory") {
            true => {
                match std::fs::create_dir_all(
                    self.args.get_one::<String>("output-directory").unwrap(),
                ) {
                    Ok(_) => {}
                    Err(err) => panic!("Unable to create output directory {:?}", err),
                };
                self.args.get_one::<String>("output-directory").unwrap()
            }
            false => "./",
        };

        pool.scoped(|scope| {
            Self::begin_tick(0, &self.progress_bars, &self.multi_inner, "");
            Self::begin_tick(1, &self.progress_bars, &self.multi_inner, "");

            for (ref_idx, reference_stem) in self.reference_map.clone().into_iter() {
                let mode = self.mode;
                let multi_inner = &self.multi_inner;
                let tree = &self.tree;
                let progress_bars = &self.progress_bars;
                let flag_filters = &self.flag_filters;
                let _reference_map = &self.reference_map;
                let _references = &self.references;
                let tmp_bam_file_cache = match self.tmp_bam_file_cache.as_ref() {
                    Some(cache) => Some(cache.path().to_str().unwrap().to_string()),
                    None => None,
                };
                let concatenated_genomes = match self.concatenated_genomes.as_ref() {
                    Some(file) => Some(file.path().to_str().unwrap().to_string()),
                    None => None,
                };
                let genomes_and_contigs = self.genomes_and_contigs.clone();

                #[cfg(feature = "fst")]
                let ploidy = *self.args.get_one::<usize>("ploidy").unwrap();

                let output_prefix = format!(
                    "{}/{}",
                    &output_prefix,
                    Path::new(&reference_stem)
                        .file_stem()
                        .unwrap()
                        .to_str()
                        .unwrap(),
                );

                if Path::new(&output_prefix).exists() && !self.args.get_flag("force") {
                    let cache = glob::glob(&format!(
                        "{}/*{}",
                        &output_prefix,
                        if mode == "call" {
                            ".vcf*"
                        } else if mode == "genotype" {
                            "strain_coverages.tsv"
                        } else if mode == "consensus" {
                            "consensus_*.fna"
                        } else {
                            ".vcf*"
                        }
                    ))
                    .expect("failed to interpret glob")
                    .map(|p| {
                        p.expect("Failed to read cached vcf path")
                            .to_str()
                            .unwrap()
                            .to_string()
                    })
                    .collect::<Vec<String>>();
                    if cache.len() > 0 {
                        if self.args.get_flag("calculate-dnds")
                            || self.args.get_flag("calculate-fst")
                        {
                            scope.execute(move || {
                                // This is here to calculate dnds values if calculate dnds is
                                // specified but not force. Kind of an edge case, but I think
                                // it could happen often. Avoids recalling variants.
                                // Needs a refactor
                                let reference = &genomes_and_contigs.genomes[ref_idx];
                                Self::begin_tick(
                                    ref_idx + 2,
                                    &progress_bars,
                                    &multi_inner,
                                    "Calculating evolutionary rates...",
                                );
                                
                                #[cfg(feature = "fst")]
                                let depth_per_sample_filter: i64 = *self
                                    .args
                                    .get_one::<i64>("depth-per-sample-filter")
                                    .unwrap();

                                let mut reference_reader = ReferenceReader::new(
                                    &Some(reference_stem.to_string()),
                                    genomes_and_contigs.clone(),
                                    genomes_and_contigs.contigs,
                                );

                                #[cfg(feature = "fst")]
                                if self.args.get_flag("calculate-fst") {
                                    {
                                        let pb = &tree.lock().unwrap()[ref_idx + 2];
                                        pb.progress_bar.set_message(format!(
                                            "{}: Calculating Fst values...",
                                            &reference_reader.genomes_and_contigs.genomes[ref_idx],
                                        ));
                                    }
                                    

                                    let mut vcf_path = format!(
                                        "{}/{}.vcf",
                                        &output_prefix,
                                        &reference_reader.genomes_and_contigs.genomes[ref_idx]
                                    );

                                    // check if we should be using gzipped vcf
                                    if !Path::new(&vcf_path).exists() {
                                        vcf_path = format!(
                                            "{}.gz",
                                            vcf_path
                                        );

                                        if !Path::new(&vcf_path).exists() {
                                            panic!("Unable to find vcf file for Fst calculation: {}", vcf_path);
                                        }
                                    }

                                    match calculate_fst(
                                        &output_prefix,
                                        &reference_reader.genomes_and_contigs.genomes[ref_idx],
                                        vcf_path.as_str(),
                                        ploidy,
                                        depth_per_sample_filter,
                                    ) {
                                        Ok(_) => {
                                            //
                                        }
                                        Err(e) => {
                                            warn!("Python error {:?}", e);
                                        }
                                    }
                                }

                                if self.args.get_flag("calculate-dnds") {
                                    {
                                        let pb = &tree.lock().unwrap()[ref_idx + 2];
                                        pb.progress_bar.set_message(format!(
                                            "{}: Calculating evolutionary rates...",
                                            &reference_reader.genomes_and_contigs.genomes[ref_idx],
                                        ));
                                    }
                                    calculate_dnds(
                                        self.args,
                                        &reference_stem,
                                        output_prefix.as_str(),
                                        &mut reference_reader,
                                        ref_idx,
                                        self.short_read_bam_count + self.long_read_bam_count,
                                    );
                                }

                                {
                                    let pb = &tree.lock().unwrap()[ref_idx + 2];
                                    pb.progress_bar.set_message(format!(
                                        "{}: All steps completed {}",
                                        &reference, "✔",
                                    ));
                                    pb.progress_bar.finish_and_clear();
                                }
                                {
                                    let pb = &tree.lock().unwrap()[1];
                                    pb.progress_bar.inc(1);
                                    let pos = pb.progress_bar.position();
                                    let len = pb.progress_bar.length().unwrap_or_else(|| 0);
                                    if pos >= len {
                                        pb.progress_bar.finish_with_message(format!(
                                            "All genomes analyzed {}",
                                            "✔",
                                        ));
                                    }
                                }
                                {
                                    let pb = &tree.lock().unwrap()[0];
                                    pb.progress_bar.inc(1);
                                    let pos = pb.progress_bar.position();
                                    let len = pb.progress_bar.length().unwrap_or_else(|| 0);
                                    if pos >= len {
                                        pb.progress_bar.finish_with_message(format!(
                                            "All steps completed {}",
                                            "✔",
                                        ));
                                    }
                                }
                            });
                            continue;
                        } else {
                            {
                                let elem = &progress_bars[ref_idx + 2];
                                let _pb = multi_inner.insert(ref_idx + 2, elem.progress_bar.clone());
                            }
                            {
                                let pb = &tree.lock().unwrap()[ref_idx + 2];

                                pb.progress_bar.set_message(format!(
                                    "{}: Output already present. Run with --force to overwrite",
                                    &genomes_and_contigs.genomes[ref_idx]
                                ));
                                pb.progress_bar.finish_and_clear();
                            }
                            {
                                let pb = &tree.lock().unwrap()[1];
                                pb.progress_bar.inc(1);
                                pb.progress_bar.reset_eta();
                                let pos = pb.progress_bar.position();
                                let len = pb.progress_bar.length().unwrap_or_else(|| 0);
                                if pos >= len {
                                    pb.progress_bar.finish_with_message(format!(
                                        "All genomes analyzed {}",
                                        "✔",
                                    ));
                                }
                            }
                            {
                                let pb = &tree.lock().unwrap()[0];
                                pb.progress_bar.inc(1);
                                pb.progress_bar.reset_eta();
                                let pos = pb.progress_bar.position();
                                let len = pb.progress_bar.length().unwrap_or_else(|| 0);
                                if pos >= len {
                                    pb.progress_bar.finish_with_message(format!(
                                        "All steps completed {}",
                                        "✔",
                                    ));
                                }
                            }
                            continue;
                        }
                    }
                }

                scope.execute(move || {
                    let reference = &genomes_and_contigs.genomes[ref_idx];
                    Self::begin_tick(
                        ref_idx + 2,
                        &progress_bars,
                        &multi_inner,
                        "Preparing variants",
                    );

                    debug!("Reference: {} {}", &reference, &reference_stem);

                    // Read BAMs back in as indexed
                    let indexed_bam_readers = recover_bams(
                        &self.args,
                        &concatenated_genomes,
                        self.short_read_bam_count,
                        self.long_read_bam_count,
                        &genomes_and_contigs,
                        n_threads as u32,
                        &tmp_bam_file_cache,
                        self.run_in_parallel,
                        // false,
                        ref_idx,
                    );

                    debug!("Indexed bam readers {:?}", &indexed_bam_readers);

                    // let mut reference_reader = ReferenceReader::new(
                    //     &Some(concatenated_genomes.as_ref().unwrap().to_string()),
                    //     genomes_and_contigs.clone(),
                    //     genomes_and_contigs.contig_to_genome.len(),
                    // );

                    let mut reference_reader = ReferenceReader::new(
                        &Some(reference_stem.to_string()),
                        genomes_and_contigs.clone(),
                        genomes_and_contigs.contigs,
                    );

                    let _per_reference_samples = 0;
                    let _per_reference_short_samples = 0;

                    if !self.args.get_flag("do-not-call-svs") && self.long_read_bam_count > 0 {
                        {
                            let pb = &tree.lock().unwrap()[ref_idx + 2];
                            pb.progress_bar
                                .set_message(format!("{}: Collecting SVs using svim...", pb.key));
                        }

                        Self::call_structural_variants(
                            &indexed_bam_readers[self.short_read_bam_count..],
                            &output_prefix,
                            concatenated_genomes.as_ref().unwrap(),
                            self.args,
                        );
                    }

                    debug!(
                        "Running SNP calling on {} samples",
                        indexed_bam_readers.len()
                    );

                    let mut assembly_engine = AssemblyRegionWalker::start(
                        self.args,
                        ref_idx,
                        self.short_read_bam_count,
                        self.long_read_bam_count,
                        &indexed_bam_readers,
                        // n_threads,
                    );

                    {
                        let pb = &tree.lock().unwrap()[ref_idx + 2];
                        pb.progress_bar.set_message(format!(
                            "{}: Performing variant calling on active regions...",
                            pb.key
                        ));
                    }

                    let (mut contexts, passing_sites) = assembly_engine.collect_shards(
                        self.args,
                        &indexed_bam_readers,
                        &genomes_and_contigs,
                        &concatenated_genomes,
                        flag_filters,
                        n_threads,
                        &mut reference_reader,
                        &output_prefix,
                        ref_idx + 2,
                        &tree
                    );

                    let genome_size = reference_reader
                        .target_lens
                        .iter()
                        .map(|(_, length)| length)
                        .sum::<u64>();

                    contexts.par_sort_unstable();
                    // contexts.reverse();
                    debug!("example variant {:?}", &contexts.first());

                    let cleaned_sample_names = get_cleaned_sample_names(&indexed_bam_readers);

                    // ensure output path exists
                    create_dir_all(&output_prefix).expect("Unable to create output directory");

                    let qual_by_depth_filter: f64 = *self
                        .args
                        .get_one::<f64>("qual-by-depth-filter")
                        .unwrap();

                    let depth_per_sample_filter: i64 = *self
                        .args
                        .get_one::<i64>("depth-per-sample-filter")
                        .unwrap();

                    let qual_filter = *self
                        .args
                        .get_one::<f64>("qual-threshold")
                        .unwrap()
                        / -10.0;

                    #[cfg(feature = "fst")]
                    let vcf_path = format!(
                        "{}/{}.vcf",
                        &output_prefix, &reference_reader.genomes_and_contigs.genomes[ref_idx]
                    );
                    if mode == "call" {
                        // calculate ANI statistics for short reads only
                        {
                            let pb = &tree.lock().unwrap()[ref_idx + 2];
                            pb.progress_bar.set_message(format!(
                                "{}: Running ANI calculations...",
                                pb.key
                            ));
                        }
                        let mut ani_calculator = ANICalculator::new(
                            self.short_read_bam_count + self.long_read_bam_count,
                        );
                        ani_calculator.run_calculator(
                            &mut contexts,
                            &output_prefix,
                            &cleaned_sample_names,
                            reference,
                            genome_size,
                            Some(passing_sites),
                            qual_by_depth_filter,
                            qual_filter,
                            depth_per_sample_filter,
                        );

                        {
                            let pb = &tree.lock().unwrap()[ref_idx + 2];
                            pb.progress_bar.set_message(format!(
                                "{}: Generating VCF file of {} variant positions...",
                                &reference,
                                contexts.len()
                            ));
                        }
                        assembly_engine.evaluator.write_vcf(
                            &output_prefix,
                            &contexts,
                            &cleaned_sample_names,
                            &reference_reader,
                            false,
                        );

                        #[cfg(feature = "fst")]
                        if self.args.get_flag("calculate-fst") {
                            {
                                let pb = &tree.lock().unwrap()[ref_idx + 2];
                                pb.progress_bar.set_message(format!(
                                    "{}: Calculating Fst values...",
                                    &reference,
                                ));
                            }
                            match calculate_fst(
                                &output_prefix,
                                &reference_reader.genomes_and_contigs.genomes[ref_idx],
                                vcf_path.as_str(),
                                ploidy,
                                depth_per_sample_filter,
                            ) {
                                Ok(_) => {
                                    //
                                }
                                Err(e) => {
                                    warn!("Python error {:?}", e);
                                }
                            }
                        }

                        if self.args.get_flag("calculate-dnds") {
                            {
                                let pb = &tree.lock().unwrap()[ref_idx + 2];
                                pb.progress_bar.set_message(format!(
                                    "{}: Calculating evolutionary rates...",
                                    &reference,
                                ));
                            }
                            calculate_dnds(
                                self.args,
                                &reference_stem,
                                output_prefix.as_str(),
                                &mut reference_reader,
                                ref_idx,
                                cleaned_sample_names.len(),
                            );
                        }
                    } else if mode == "genotype" {
                        // If a variant context contains more than one allele, we need to split
                        // this context into n different contexts, where n is number of variant
                        // alleles
                        let (mut split_contexts, filtered_contexts) =
                            VariantContextUtils::split_contexts(
                                contexts,
                                qual_by_depth_filter,
                                *self.args
                                    .get_one::<i64>("min-variant-depth-for-genotyping")
                                    .unwrap() as i32,
                            );
                        
                        {
                            let pb = &tree.lock().unwrap()[ref_idx + 2];
                            pb.progress_bar.set_message(format!(
                                "{}: Running ANI calculations...",
                                pb.key
                            ));
                        }
                        // calculate ANI statistics
                        let mut ani_calculator = ANICalculator::new(
                            self.short_read_bam_count + self.long_read_bam_count,
                        );
                        ani_calculator.run_calculator(
                            &mut split_contexts,
                            &output_prefix,
                            &cleaned_sample_names,
                            reference,
                            genome_size,
                            Some(passing_sites),
                            qual_by_depth_filter,
                            qual_filter,
                            depth_per_sample_filter,
                        );

                        if split_contexts.len() >= 1 {
                            // Perform UMAP and HDBSCAN clustering followed by variant group
                            // read linkage clustering.
                            let clustering_engine = HaplotypeClusteringEngine::new(
                                output_prefix.as_str(),
                                split_contexts,
                                &reference_reader,
                                ref_idx,
                                indexed_bam_readers.len(),
                                n_threads,
                            );
                            let (n_strains, split_contexts) = clustering_engine.perform_clustering(
                                &indexed_bam_readers,
                                flag_filters,
                                n_threads,
                                tree,
                            );
                            debug!(
                                "example variant after clustering {:?}",
                                &split_contexts.first()
                            );

                            // Get strain abundances
                            {
                                let pb = &tree.lock().unwrap()[ref_idx + 2];
                                pb.progress_bar.set_message(format!(
                                    "{}: Calculating genotype abundances...",
                                    &reference,
                                ));
                            }
                            let abundance_calculator_engine = AbundanceCalculatorEngine::new(
                                split_contexts,
                                &reference_reader.genomes_and_contigs.genomes[ref_idx],
                                &output_prefix,
                                &cleaned_sample_names,
                            );

                            let (strain_ids_present, mut split_contexts) =
                                abundance_calculator_engine.run_abundance_calculator(
                                    n_strains,
                                    cleaned_sample_names.len(),
                                );

                            // let strain_ids_present = (0..n_strains).into_iter().collect::<Vec<usize>>();
                            {
                                let pb = &tree.lock().unwrap()[ref_idx + 2];
                                pb.progress_bar
                                    .set_message(
                                        format!("{}: Generating VCF file...", &reference,),
                                    );
                            }

                            split_contexts.extend(filtered_contexts);
                            split_contexts.par_sort_unstable();
                            assembly_engine.evaluator.write_vcf(
                                &output_prefix,
                                &split_contexts,
                                &cleaned_sample_names,
                                &reference_reader,
                                true,
                            );

                            #[cfg(feature = "fst")]
                            if self.args.get_flag("calculate-fst") {
                                {
                                    let pb = &tree.lock().unwrap()[ref_idx + 2];
                                    pb.progress_bar.set_message(format!(
                                        "{}: Calculating Fst values...",
                                        &reference,
                                    ));
                                }
                                match calculate_fst(
                                    &output_prefix,
                                    &reference_reader.genomes_and_contigs.genomes[ref_idx],
                                    vcf_path.as_str(),
                                    ploidy,
                                    depth_per_sample_filter,
                                ) {
                                    Ok(_) => {
                                        //
                                    }
                                    Err(e) => {
                                        warn!("Python error {:?}", e);
                                    }
                                }
                            }

                            if self.args.get_flag("calculate-dnds") {
                                {
                                    let pb = &tree.lock().unwrap()[ref_idx + 2];
                                    pb.progress_bar.set_message(format!(
                                        "{}: Calculating evolutionary rates...",
                                        &reference,
                                    ));
                                }
                                calculate_dnds(
                                    self.args,
                                    &reference_stem,
                                    output_prefix.as_str(),
                                    &mut reference_reader,
                                    ref_idx,
                                    cleaned_sample_names.len(),
                                );
                            }

                            // Write genotypes to disk, reference specific
                            {
                                let pb = &tree.lock().unwrap()[ref_idx + 2];
                                pb.progress_bar
                                    .set_message(format!("{}: Writing strains...", &reference,));
                            }
                            let mut reference_writer =
                                ReferenceWriter::new(reference_reader, &output_prefix);
                            reference_writer.generate_strains(
                                split_contexts,
                                ref_idx,
                                if strain_ids_present.len() > 0 {
                                    strain_ids_present
                                } else {
                                    vec![0]
                                },
                            );
                        } else {
                            split_contexts.extend(filtered_contexts);
                            assembly_engine.evaluator.write_vcf(
                                &output_prefix,
                                &split_contexts,
                                &cleaned_sample_names,
                                &reference_reader,
                                true,
                            );

                            #[cfg(feature = "fst")]
                            if self.args.get_flag("calculate-fst") {
                                {
                                    let pb = &tree.lock().unwrap()[ref_idx + 2];
                                    pb.progress_bar.set_message(format!(
                                        "{}: Calculating Fst values...",
                                        &reference,
                                    ));
                                }
                                match calculate_fst(
                                    &output_prefix,
                                    &reference_reader.genomes_and_contigs.genomes[ref_idx],
                                    vcf_path.as_str(),
                                    ploidy,
                                    depth_per_sample_filter,
                                ) {
                                    Ok(_) => {
                                        //
                                    }
                                    Err(e) => {
                                        warn!("Python error {:?}", e);
                                    }
                                }
                            }

                            if self.args.get_flag("calculate-dnds") {
                                {
                                    let pb = &tree.lock().unwrap()[ref_idx + 2];
                                    pb.progress_bar.set_message(format!(
                                        "{}: Calculating evolutionary rates...",
                                        &reference,
                                    ));
                                }
                                calculate_dnds(
                                    self.args,
                                    &reference_stem,
                                    output_prefix.as_str(),
                                    &mut reference_reader,
                                    ref_idx,
                                    cleaned_sample_names.len(),
                                );
                            }
                            // Write genotypes to disk, reference specific
                            {
                                let pb = &tree.lock().unwrap()[ref_idx + 2];
                                pb.progress_bar.set_message(format!(
                                    "{}: Writing reference strain...",
                                    &reference,
                                ));
                            }
                            let mut reference_writer =
                                ReferenceWriter::new(reference_reader, &output_prefix);
                            reference_writer.generate_strains(split_contexts, ref_idx, vec![0]);
                        }
                    } else if mode == "consensus" {
                        {
                            let pb = &tree.lock().unwrap()[ref_idx + 2];
                            pb.progress_bar.set_message(format!(
                                "{}: Running ANI calculations...",
                                pb.key
                            ));
                        }
                        // calculate ANI statistics
                        let mut ani_calculator = ANICalculator::new(
                            self.short_read_bam_count + self.long_read_bam_count,
                        );
                        ani_calculator.run_calculator(
                            &mut contexts,
                            &output_prefix,
                            &cleaned_sample_names,
                            reference,
                            genome_size,
                            Some(passing_sites),
                            qual_by_depth_filter,
                            qual_filter,
                            depth_per_sample_filter,
                        );
                        // Get sample distances
                        {
                            let pb = &tree.lock().unwrap()[ref_idx + 2];
                            pb.progress_bar
                                .set_message(format!("{}: Generating VCF file...", &reference,));
                        }
                        assembly_engine.evaluator.write_vcf(
                            &output_prefix,
                            &contexts,
                            &cleaned_sample_names,
                            &reference_reader,
                            false,
                        );

                        #[cfg(feature = "fst")]
                        if self.args.get_flag("calculate-fst") {
                            {
                                let pb = &tree.lock().unwrap()[ref_idx + 2];
                                pb.progress_bar.set_message(format!(
                                    "{}: Calculating Fst values...",
                                    &reference,
                                ));
                            }
                            match calculate_fst(
                                &output_prefix,
                                &reference_reader.genomes_and_contigs.genomes[ref_idx],
                                vcf_path.as_str(),
                                ploidy,
                                depth_per_sample_filter,
                            ) {
                                Ok(_) => {
                                    //
                                }
                                Err(e) => {
                                    warn!("Python error {:?}", e);
                                }
                            }
                        }

                        if self.args.get_flag("calculate-dnds") {
                            {
                                let pb = &tree.lock().unwrap()[ref_idx + 2];
                                pb.progress_bar.set_message(format!(
                                    "{}: Calculating evolutionary rates...",
                                    &reference,
                                ));
                            }
                            calculate_dnds(
                                self.args,
                                &reference_stem,
                                output_prefix.as_str(),
                                &mut reference_reader,
                                ref_idx,
                                cleaned_sample_names.len(),
                            );
                        }

                        {
                            let pb = &tree.lock().unwrap()[ref_idx + 2];
                            pb.progress_bar.set_message(format!(
                                "{}: Generating consensus genomes...",
                                &reference,
                            ));
                        }
                        // variant_matrix.generate_distances();
                        let mut reference_writer =
                            ReferenceWriter::new(reference_reader, &output_prefix);
                        reference_writer.generate_consensus(
                            contexts,
                            ref_idx,
                            &cleaned_sample_names,
                        );
                    };

                    {
                        let pb = &tree.lock().unwrap()[ref_idx + 2];
                        pb.progress_bar
                            .set_message(format!("{}: All steps completed {}", &reference, "✔",));
                        pb.progress_bar.finish_and_clear();
                    }
                    {
                        let pb = &tree.lock().unwrap()[1];
                        pb.progress_bar.inc(1);
                        let pos = pb.progress_bar.position();
                        let len = pb.progress_bar.length().unwrap_or_else(|| 0);
                        if pos >= len {
                            pb.progress_bar
                                .finish_with_message(format!("All genomes analyzed {}", "✔",));
                        }
                    }
                    {
                        let pb = &tree.lock().unwrap()[0];
                        pb.progress_bar.inc(1);
                        let pos = pb.progress_bar.position();
                        let len = pb.progress_bar.length().unwrap_or_else(|| 0);
                        if pos >= len {
                            pb.progress_bar
                                .finish_with_message(format!("All steps completed {}", "✔",));
                        }
                    }
                });
            }

            // self.multi.join().unwrap();
        });
    }

    /// Uses svim to call potential structural variants along the current reference genome
    /// Any retrieved structural variants are stored in their own VCF file but also
    /// used as `feature` variants to guide potential short read calls of these variants
    fn call_structural_variants(
        indexed_longread_bam_readers: &[String],
        output_prefix: &str,
        reference: &str,
        args: &clap::ArgMatches,
    ) {
        check_for_svim();
        check_for_bcftools();
        let min_mapq = args.get_one::<u8>("min-mapq").unwrap();
        let min_sv_qual = args.get_one::<u8>("min-sv-qual").unwrap();
        debug!("bam readers {:?}", indexed_longread_bam_readers);
        // use svim on each longread sample
        indexed_longread_bam_readers
            .into_par_iter()
            .enumerate()
            .for_each(|(idx, bam_reader)| {

                // svim path is just output prefix with numbered svim
                let svim_path = format!("{}/svim_{}", output_prefix, idx);

                let cmd_string = format!(
                    "set -e -o pipefail; \
                    svim alignment \
                    --skip_genotyping \
                    --min_mapq {} --sequence_alleles \
                    {} {} {}; \
                    bcftools sort {}/variants.vcf | bcftools view -i 'QUAL >= {}' > {}/variants_filtered_sorted.vcf; \
                    bgzip {}/variants_filtered_sorted.vcf; bcftools index {}/variants_filtered_sorted.vcf.gz",
                    min_mapq,
                    &svim_path,
                    bam_reader,
                    reference,
                    &svim_path,
                    &min_sv_qual,
                    &svim_path,
                    &svim_path,
                    &svim_path,
                );

                debug!("Queuing cmd string {}", &cmd_string);

                // We do not want to capture any stdio from svim as it produces too much
                // and we can't clear the buffer before it starts hanging: https://github.com/rust-lang/rust/issues/45572
                finish_command_safely(
                    Command::new("bash")
                        .arg("-c")
                        .arg(&cmd_string)
                        .stderr(Stdio::null())
                        .spawn()
                        .expect("Unable to execute svim command"),
                    "svim"
                );
        });

        if indexed_longread_bam_readers.len() > 1 {
            // once svim has run on each sample, we need to merge the VCF files together
            // the easiest way to do this is bcftools merge
            let cmd_string = format!(
                "set -e -o pipefail; \
                bcftools merge {}/svim_*/variants_filtered_sorted.vcf.gz | bcftools sort > {}/structural_variants.vcf; \
                bgzip {}/structural_variants.vcf; bcftools index {}/structural_variants.vcf.gz",
                output_prefix,
                output_prefix,
                output_prefix,
                output_prefix
            );

            debug!("Queuing cmd string {}", &cmd_string);
            finish_command_safely(
                Command::new("bash")
                    .arg("-c")
                    .arg(&cmd_string)
                    .stderr(Stdio::piped())
                    .spawn()
                    .expect("Unable to execute bcftools command"),
                "bcftools",
            );
        } else {
            // if there is only one longread sample just use that one
            let cmd_string = format!(
                "set -e -o pipefail; \
                mv {}/svim_0/variants_filtered_sorted.vcf.gz {}/structural_variants.vcf.gz; \
                bcftools index {}/structural_variants.vcf.gz",
                output_prefix, output_prefix, output_prefix
            );

            debug!("Queuing cmd string {}", &cmd_string);
            finish_command_safely(
                Command::new("bash")
                    .arg("-c")
                    .arg(&cmd_string)
                    .stderr(Stdio::piped())
                    .spawn()
                    .expect("Unable to execute bcftools command"),
                "mv",
            );
        }
    }

    pub fn setup_progress_bars(
        references: &Vec<&str>,
        reference_map: &mut HashMap<usize, String>,
        genomes_and_contigs: &GenomesAndContigs,
        short_sample_count: usize,
        long_sample_count: usize,
    ) -> Result<Vec<Elem>, TemplateError> {
        // Put reference index in the variant map and initialize matrix
        let mut progress_bars = vec![
            Elem {
                key: "Genomes complete".to_string(),
                index: 1,
                progress_bar: ProgressBar::new(references.len() as u64),
            };
            references.len() + 2
        ];

        for reference in references.iter() {
            debug!(
                "Genomes {:?} contigs {:?}",
                &genomes_and_contigs.genomes, &genomes_and_contigs.contigs,
            );

            let ref_idx = genomes_and_contigs
                .genome_index(
                    &Path::new(reference)
                        .file_stem()
                        .expect("problem determining file stem")
                        .to_str()
                        .unwrap()
                        .to_string(),
                )
                .unwrap();

            progress_bars[ref_idx + 2] = Elem {
                key: genomes_and_contigs.genomes[ref_idx].clone(),
                index: ref_idx,
                progress_bar: ProgressBar::new((short_sample_count + long_sample_count) as u64 + 1),
            };
            debug!("Reference {}", reference,);
            reference_map
                .entry(ref_idx)
                .or_insert_with(|| reference.to_string());
        }

        progress_bars[0] = Elem {
            key: "Operations remaining".to_string(),
            index: 0,
            progress_bar: ProgressBar::new(
                ((references.len() * (short_sample_count + long_sample_count)) + references.len())
                    as u64,
            ),
        };

        let sty_eta = ProgressStyle::default_bar().template(
            "[{elapsed_precise}] {bar:40.cyan/blue} {pos:>7}/{len:7} {msg}",
        )?;

        let sty_aux = ProgressStyle::default_bar()
            .template("[{elapsed_precise}] {spinner:.green} {msg} {pos:>4}/{len:4}")?;
        progress_bars
            .par_iter()
            .for_each(|pb| pb.progress_bar.set_style(sty_aux.clone()));
        progress_bars[0].progress_bar.set_style(sty_eta);

        return Ok(progress_bars);
    }

    pub fn begin_tick(
        index: usize,
        progress_bars: &Vec<Elem>,
        multi_inner: &Arc<MultiProgress>,
        message: &str,
    ) {
        let elem = &progress_bars[index];
        let pb = multi_inner.insert(index, elem.progress_bar.clone());

        pb.enable_steady_tick(Duration::from_millis(200));

        pb.set_message(format!("{}: {}...", &elem.key, message));
    }
}

pub fn start_lorikeet_engine<
    R: NamedBamReader,
    S: NamedBamReaderGenerator<R>,
    T: NamedBamReader,
    U: NamedBamReaderGenerator<T>,
>(
    m: &clap::ArgMatches,
    bam_readers: Vec<S>,
    longreads: Option<Vec<U>>,
    mode: &str,
    flag_filters: FlagFilter,
    genomes_and_contigs: GenomesAndContigs,
    tmp_bam_file_cache: Option<TempDir>,
    concatenated_genomes: Option<NamedTempFile>,
) -> Result<(), BirdToolError> {
    let threads = match m.get_one::<usize>("threads") {
        Some(val) => *val,
        None => {
            return Err(BirdToolError::DebugError(
                "Failed to parse number of threads.".to_string(),
            ))
        }
    };
    debug!("Parsing reference info...");
    let references = ReferenceReaderUtils::parse_references(&m);
    debug!("Parsing reference info...done. ({} references)", references.len());
    let references = references.par_iter().map(|p| &**p).collect::<Vec<&str>>();
    debug!("Retrieving references...");
    let concatenated_temp_file_name = match concatenated_genomes {
        Some(ref file) => file.path().to_str().unwrap().to_string(),
        None => "".to_string(),
    };
    ReferenceReaderUtils::retrieve_reference(&Some(
        concatenated_temp_file_name.clone(),
    ));

    // All different counts of samples I need. Changes depends on when using concatenated genomes or not
    let short_read_bam_count = bam_readers.len();
    let mut long_read_bam_count = 0;
    let reference_count = references.len();

    let longreads = match longreads {
        Some(vec) => {
            long_read_bam_count += vec.len();
            vec
        }
        None => vec![],
    };


    // Finish each BAM source
    if m.contains_id("longreads") || m.contains_id("longread-bam-files") {
        info!("Processing long reads...");
        finish_bams(
            longreads,
            threads,
            &genomes_and_contigs,
            // run_in_parallel,
            m.get_flag("split-bams"),
            !m.contains_id("longread-bam-files"),
        ).expect("Failed to finish BAMs");
    }

    if m.contains_id("coupled")
        || m.contains_id("interleaved")
        || m.contains_id("read1")
        || m.contains_id("read2")
        || m.contains_id("single")
        || m.contains_id("bam-files")
    {
        info!("Processing short reads...");
        finish_bams(
            bam_readers,
            threads,
            &genomes_and_contigs,
            // run_in_parallel,
            m.get_flag("split-bams"),
            !m.contains_id("bam-files"),
        ).expect("Failed to finish BAMs");
    }

    let mut reference_map = HashMap::new();

    // Set up multi progress bars
    let multi = Arc::new(MultiProgress::new());

    let multi_inner = Arc::clone(&multi);
    let progress_bars = match LorikeetEngine::setup_progress_bars(
        &references,
        &mut reference_map,
        &genomes_and_contigs,
        // short_read_bam_count,
        // long_read_bam_count,
        0,
        0,
    ) {
        Ok(val) => val,
        Err(e) => return Err(BirdToolError::DebugError(e.to_string())),
    };

    let tree: Arc<Mutex<Vec<&Elem>>> =
        Arc::new(Mutex::new(Vec::with_capacity(progress_bars.len())));
    {
        let mut tree = tree.lock().unwrap();
        for pb in progress_bars.iter() {
            tree.push(pb)
        }
    }

    debug!(
        "{} Longread BAM files, {} Shortread BAM files {} Total BAMs over {} genome(s)",
        long_read_bam_count,
        short_read_bam_count,
        (short_read_bam_count + long_read_bam_count),
        reference_count
    );

    {
        let lorikeet_engine = LorikeetEngine {
            args: m,
            short_read_bam_count,
            long_read_bam_count,
            flag_filters,
            genomes_and_contigs,
            concatenated_genomes,
            tmp_bam_file_cache,
            reference_map,
            references,
            // multi,
            multi_inner,
            tree,
            progress_bars: &progress_bars,
            threads,
            mode,
            run_in_parallel: m.get_flag("split-bams"),
        };

        lorikeet_engine.apply_per_reference();
    }

    // cleanup temp files .fai index file
    if Path::new(format!("{}.fai", concatenated_temp_file_name).as_str()).exists() {
        std::fs::remove_file(format!("{}.fai", concatenated_temp_file_name).as_str())
            .expect("Failed to remove temp file");
    }

    Ok(())
}

pub fn run_summarize(args: &clap::ArgMatches) {
    let vcf_files = args.get_many::<String>("vcfs").unwrap().map(|s| &**s).collect::<Vec<&str>>();
    let qual_by_depth_filter = *args
        .get_one::<f64>("qual-by-depth-filter")
        .unwrap();
    let qual_filter = *args
        .get_one::<f64>("qual-threshold")
        .unwrap()
        / -10.0;
    let depth_per_sample_filter: i64 = *args
        .get_one::<i64>("depth-per-sample-filter")
        .unwrap();

    let output_prefix = match args.contains_id("output") {
        true => {
            match std::fs::create_dir_all(args.get_one::<String>("output").unwrap().to_string()) {
                Ok(_) => {}
                Err(err) => panic!("Unable to create output directory {:?}", err),
            };
            args.get_one::<String>("output").unwrap()
        }
        false => "./",
    };

    vcf_files.into_iter().for_each(|vcf_path| {
        let reader = rust_htslib::bcf::Reader::from_path(vcf_path).unwrap();
        let header = reader.header();
        let mut variant_contexts = VariantContext::process_vcf_from_path(vcf_path, true);

        #[cfg(feature = "fst")]
        let mut ploidy = 2;

        // workout ploidy
        #[cfg(feature = "fst")]
        match variant_contexts.first_mut() {
            Some(record) => ploidy = record.genotypes.get_max_ploidy(2),
            None => {}
        }
        let samples: Vec<&str> = header
            .samples()
            .into_iter()
            .map(|s| std::str::from_utf8(s).unwrap())
            .collect::<Vec<&str>>();

        let genome_size: u64 = header
            .header_records()
            .into_iter()
            .map(|h_record| match h_record {
                rust_htslib::bcf::header::HeaderRecord::Contig { key: _, values } => {
                    let size = values.get("length").unwrap();
                    let size: u64 = size.parse().unwrap();
                    size
                }
                _ => 0,
            })
            .sum();
        // calculate ANI statistics
        let mut ani_calculator = ANICalculator::new(variant_contexts[0].genotypes.len());
        ani_calculator.run_calculator(
            &mut variant_contexts,
            output_prefix,
            samples.as_slice(),
            Path::new(vcf_path).file_stem().unwrap().to_str().unwrap(),
            genome_size,
            None,
            qual_by_depth_filter,
            qual_filter,
            depth_per_sample_filter,
        );

        #[cfg(feature = "fst")]
        calculate_fst(
            output_prefix,
            Path::new(vcf_path).file_stem().unwrap().to_str().unwrap(),
            vcf_path,
            ploidy as usize,
            depth_per_sample_filter,
        ).expect("Failed to calculate FST");
    })
}

/// Checks for the presence of gff file in the output directory for the current reference
/// If none is present then generate one
fn check_for_gff(
    reference: &str,
    output_prefix: &str,
    m: &clap::ArgMatches,
) -> Option<bio::io::gff::Reader<File>> {
    let cache = glob::glob(&format!("{}/*.gff", &output_prefix))
        .expect("failed to interpret glob")
        .map(|p| {
            p.expect("Failed to read cached gff path")
                .to_str()
                .unwrap()
                .to_string()
        })
        .collect::<Vec<String>>();

    if cache.len() > 1 {
        debug!("Too many GFF files in output folder: {}", output_prefix);
        return None;
    } else if cache.len() == 1 {
        debug!("Reading cached gff file: {}", &cache[0]);
        // Read in previous gff file
        let gff_reader = bio::io::gff::Reader::from_file(&cache[0], bio::io::gff::GffType::GFF3)
            .expect("Failed to read GFF file");
        Some(gff_reader)
    } else {
        let gff_path = format!("{}/genes.gff", output_prefix);
        let cmd_string = format!(
            "set -e -o pipefail; \
            prodigal -o {} -i {} -f gff {}",
            // prodigal
            &gff_path,
            &reference,
            m.get_one::<String>("prodigal-params").map(|s| &**s).unwrap_or_else(|| ""),
        );
        // debug!("Queuing cmd_string: {}", cmd_string);
        finish_command_safely(
            std::process::Command::new("bash")
                .arg("-c")
                .arg(&cmd_string)
                .stdout(Stdio::piped())
                .stderr(Stdio::piped())
                .spawn()
                .expect("Unable to execute bash"),
            "prodigal",
        );

        // Read in newly created gff
        let gff_reader = bio::io::gff::Reader::from_file(gff_path, bio::io::gff::GffType::GFF3)
            .expect("Failed to read GFF file");
        Some(gff_reader)
    }
}

fn calculate_dnds(
    args: &clap::ArgMatches,
    reference: &str,
    output_prefix: &str,
    reference_reader: &mut ReferenceReader,
    ref_idx: usize,
    sample_count: usize,
) {
    let qual_by_depth_filter: f64 = *args
        .get_one::<f64>("qual-by-depth-filter")
        .unwrap();

    let depth_per_sample_filter: i64 = *args
        .get_one::<i64>("depth-per-sample-filter")
        .unwrap();

    let qual_filter = *args
        .get_one::<f64>("qual-threshold")
        .unwrap()
        / -10.0;

    match check_for_gff(reference, output_prefix, args) {
        Some(mut genes) => {

            let mut vcf_prefix = format!(
                "{}/{}.vcf",
                &output_prefix,
                &reference_reader.genomes_and_contigs.genomes[ref_idx]
            );

            // check if we should be using gzipped vcf
            if !Path::new(&vcf_prefix).exists() {
                vcf_prefix = format!(
                    "{}.gz",
                    vcf_prefix
                );

                if !Path::new(&vcf_prefix).exists() {
                    panic!("Unable to find vcf file for Fst calculation: {}", vcf_prefix);
                }
            }

            debug!("Reading VCF: {}", &vcf_prefix);
            let mut variants = VariantContext::get_vcf_reader(vcf_prefix.as_str());
            debug!("Success!");
            let mut dnds_calculator = CodonTable::setup();
            dnds_calculator.get_codon_table(11);

            // create new TSV file that will contain gene\tSNPs\tindels\tdN/dS
            let tsv_file = OpenOptions::new()
                .create(true)
                .write(true)
                .open(format!(
                    "{}/{}_dnds.tsv",
                    output_prefix, &reference_reader.genomes_and_contigs.genomes[ref_idx]
                )).unwrap();
            let mut tsv_writer = BufWriter::new(tsv_file);
            // write header
            tsv_writer
                .write_all(
                    format!(
                        "contig\tID\tstart\tstop\tSNPs\tindels\tdN/dS\n",
                    ).as_bytes(),
                ).expect("Unable to write to TSV file");

            for gene in genes.records() {
                match gene {
                    Ok(gene) => {
                        let (snps, frameshifts, dnds_values) = dnds_calculator.find_mutations(
                            &gene,
                            &mut variants,
                            reference_reader,
                            ref_idx,
                            sample_count,
                            qual_by_depth_filter,
                            qual_filter,
                            depth_per_sample_filter,
                        );
                        if snps.iter().sum::<usize>() == 0 && frameshifts.iter().sum::<usize>() == 0 {
                            continue;
                        }

                        // get the ID from the attributes
                        let id = gene
                            .attributes()
                            .get("ID")
                            .expect("Unable to get ID from GFF file")
                            .to_string();

                        // write to TSV file
                        tsv_writer
                            .write_all(
                                format!(
                                    "{}\t{}\t{}\t{}\t{}\t{}\t{}\n",
                                    gene.seqname(),
                                    id,
                                    gene.start(),
                                    gene.end(),
                                    snps.into_iter().map(|s| format!("{}", s)).join(","),
                                    frameshifts.into_iter().map(|s| format!("{}", s)).join(","),
                                    dnds_values.into_iter().map(|s| format!("{}", s)).join(","),
                                ).as_bytes(),
                            ).expect("Unable to write to TSV file");
                    }
                    Err(_) => continue,
                }
            }
            tsv_writer.flush().expect("Unable to flush TSV writer");
        }
        None => {
            // too many GFF files in output folder, abort this genome
            debug!("Not calculating evolutionary rates for {} as their are too many GFF files in output folder: {}", &reference, &output_prefix);
        }
    };

    // let placeholder_gene_file = format!("{}/genes.gff", output_prefix);
    // if Path::new(&placeholder_gene_file).exists() {
    //     std::fs::remove_file(&placeholder_gene_file).expect("Unable to remove placeholder gene file");
    // }
}
