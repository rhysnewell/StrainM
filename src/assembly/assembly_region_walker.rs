use ndarray::Array2;
use rayon::prelude::*;
use rust_htslib::bcf::Read;
use std::path::Path;
use std::sync::{Arc, Mutex};


use crate::bam_parsing::FlagFilter;
use crate::activity_profile::activity_profile::Profile;
use crate::activity_profile::band_pass_activity_profile::BandPassActivityProfile;
use crate::assembly::assembly_region::AssemblyRegion;
use crate::assembly::assembly_region_iterator::AssemblyRegionIterator;
use crate::processing::lorikeet_engine::Elem;
use crate::reference::reference_reader_utils::GenomesAndContigs;
use crate::haplotype::haplotype_caller_engine::HaplotypeCallerEngine;
use crate::model::variant_context::VariantContext;
use crate::reference::reference_reader::ReferenceReader;
use crate::utils::interval_utils::IntervalUtils;
use crate::utils::simple_interval::{Locatable, SimpleInterval};

pub struct AssemblyRegionWalker {
    pub(crate) evaluator: HaplotypeCallerEngine,
    short_read_bam_count: usize,
    long_read_bam_count: usize,
    ref_idx: usize,
    assembly_region_padding: usize,
    min_assembly_region_size: usize,
    max_assembly_region_size: usize,
    // n_threads: u32,
}

impl AssemblyRegionWalker {
    pub fn start(
        args: &clap::ArgMatches,
        ref_idx: usize,
        short_read_bam_count: usize,
        long_read_bam_count: usize,
        indexed_bam_readers: &[String],
        // n_threads: usize,
    ) -> AssemblyRegionWalker {
        let hc_engine = HaplotypeCallerEngine::new(
            args,
            ref_idx,
            indexed_bam_readers.to_vec(),
            false,
            *args.get_one::<usize>("ploidy").unwrap(),
        );

        let assembly_region_padding = *args
            .get_one::<usize>("assembly-region-padding")
            .unwrap();
        let min_assembly_region_size = *args
            .get_one::<usize>("min-assembly-region-size")
            .unwrap();
        let max_assembly_region_size = *args
            .get_one::<usize>("max-assembly-region-size")
            .unwrap();

        AssemblyRegionWalker {
            evaluator: hc_engine,
            short_read_bam_count,
            long_read_bam_count,
            ref_idx,
            assembly_region_padding,
            min_assembly_region_size,
            max_assembly_region_size,
            // n_threads: n_threads as u32,
        }
    }

    pub fn collect_shards(
        &mut self,
        args: &clap::ArgMatches,
        indexed_bam_readers: &[String],
        genomes_and_contigs: &GenomesAndContigs,
        concatenated_genomes: &Option<String>,
        flag_filters: &FlagFilter,
        n_threads: usize,
        reference_reader: &mut ReferenceReader,
        output_prefix: &str,
        pb_index: usize,
        pb_tree: &Arc<Mutex<Vec<&Elem>>>
    ) -> (Vec<VariantContext>, Array2<f32>) {
        self.evaluator.collect_activity_profile(
            indexed_bam_readers,
            self.short_read_bam_count,
            // self.long_read_bam_count,
            0,
            n_threads,
            self.ref_idx,
            args,
            genomes_and_contigs,
            concatenated_genomes,
            flag_filters,
            reference_reader,
            self.assembly_region_padding,
            self.min_assembly_region_size,
            self.max_assembly_region_size,
            self.short_read_bam_count,
            self.long_read_bam_count,
            *args.get_one::<usize>("max-input-depth").unwrap(),
            output_prefix,
            pb_index,
            pb_tree
        )
    }

    pub fn process_shard<'a, 'b>(
        shard: BandPassActivityProfile,
        flag_filters: &'a FlagFilter,
        args: &clap::ArgMatches,
        sample_names: &'a [String],
        reference_reader: &ReferenceReader,
        n_threads: u32,
        assembly_region_padding: usize,
        min_assembly_region_size: usize,
        max_assembly_region_size: usize,
        short_read_bam_count: usize,
        long_read_bam_count: usize,
        evaluator: &HaplotypeCallerEngine,
        max_input_depth: usize,
        output_prefix: &'a str,
    ) -> Vec<VariantContext> {
        let assembly_region_iter = AssemblyRegionIterator::new(sample_names, n_threads);

        let pending_regions = shard.pop_ready_assembly_regions(
            assembly_region_padding,
            min_assembly_region_size,
            max_assembly_region_size,
            false, // not used, calculated in function
        );

        let features = args.get_one::<String>("features-vcf");
        let limiting_interval = IntervalUtils::parse_limiting_interval(args);
        match features {
            Some(indexed_vcf_reader) => {
                // debug!("Attempting to extract features...");

                let contexts = pending_regions
                    .into_par_iter()
                    .flat_map(|mut assembly_region| {
                        let within_bounds = match &limiting_interval {
                            Some(limit) => {
                                let limit = SimpleInterval::new(
                                    assembly_region.tid,
                                    limit.start,
                                    limit.end,
                                );
                                assembly_region.padded_span.overlaps(&limit)
                            }
                            None => true,
                        };

                        if within_bounds {
                            let mut reference_reader = reference_reader.clone();
                            let mut evaluator = evaluator.clone();

                            // read in feature variants across the assembly region location
                            let feature_variants = retrieve_feature_variants(
                                indexed_vcf_reader,
                                &reference_reader,
                                &assembly_region,
                            );

                            // if long_read_bam_count > 0 && !args.is_present("do-not-call-svs") {
                            //     let sv_path = format!("{}/structural_variants.vcf.gz", output_prefix);
                            //     if Path::new(&sv_path).exists() {
                            //         // structural variants present so we will add them to feature variants
                            //         let structural_variants = retrieve_feature_variants(
                            //             &sv_path,
                            //             &reference_reader,
                            //             &assembly_region,
                            //         );
                            //
                            //         feature_variants.extend(structural_variants);
                            //     }
                            // }

                            // debug!("Feature variants {:?}", &feature_variants);

                            assembly_region_iter.fill_next_assembly_region_with_reads(
                                &mut assembly_region,
                                flag_filters,
                                n_threads,
                                short_read_bam_count,
                                long_read_bam_count,
                                max_input_depth,
                                args,
                            );

                            evaluator
                                .call_region(
                                    assembly_region,
                                    &mut reference_reader,
                                    feature_variants,
                                    args,
                                    sample_names,
                                    flag_filters,
                                )
                                .into_par_iter()
                        } else {
                            Vec::new().into_par_iter()
                        }
                    })
                    .collect::<Vec<VariantContext>>();

                contexts
            }
            None => {
                let contexts = pending_regions
                    .into_par_iter()
                    .flat_map(|mut assembly_region| {
                        let within_bounds = match &limiting_interval {
                            Some(limit) => {
                                let limit = SimpleInterval::new(
                                    assembly_region.tid,
                                    limit.start,
                                    limit.end,
                                );
                                assembly_region.padded_span.overlaps(&limit)
                            }
                            None => true,
                        };

                        if within_bounds {
                            let mut reference_reader = reference_reader.clone();
                            let mut evaluator = evaluator.clone();

                            let feature_variants =
                                if long_read_bam_count > 0 && !args.get_flag("do-not-call-svs") {
                                    let sv_path =
                                        format!("{}/structural_variants.vcf.gz", output_prefix);
                                    if Path::new(&sv_path).exists() {
                                        // structural variants present so we will add them to feature variants
                                        // retrieve_feature_variants(
                                        //     &sv_path,
                                        //     &reference_reader,
                                        //     &assembly_region,
                                        // )
                                        Vec::new()
                                    } else {
                                        Vec::new()
                                    }
                                } else {
                                    Vec::new()
                                };

                            // debug!("Filling with reads...");
                            assembly_region_iter.fill_next_assembly_region_with_reads(
                                &mut assembly_region,
                                flag_filters,
                                n_threads,
                                short_read_bam_count,
                                long_read_bam_count,
                                max_input_depth,
                                args,
                            );

                            evaluator
                                .call_region(
                                    assembly_region,
                                    &mut reference_reader,
                                    feature_variants,
                                    args,
                                    sample_names,
                                    flag_filters,
                                )
                                .into_par_iter()
                        } else {
                            Vec::new().into_par_iter()
                        }
                    })
                    .collect::<Vec<VariantContext>>();

                contexts
            }
        }
    }
}

fn retrieve_feature_variants(
    indexed_vcf_reader: &str,
    reference_reader: &ReferenceReader,
    assembly_region: &AssemblyRegion,
) -> Vec<VariantContext> {
    let mut indexed_vcf_reader = VariantContext::retrieve_indexed_vcf_file(indexed_vcf_reader);
    // debug!("Retrieved indexed VCF...");

    let vcf_rid = VariantContext::get_contig_vcf_tid(
        indexed_vcf_reader.header(),
        reference_reader
            .retrieve_contig_name_from_tid(assembly_region.get_contig())
            .unwrap(),
    );
    // debug!("VCF Rid {:?}", &vcf_rid);

    match vcf_rid {
        Some(rid) => VariantContext::process_vcf_in_region(
            &mut indexed_vcf_reader,
            rid,
            assembly_region.get_start() as u64,
            assembly_region.get_end() as u64,
        ),
        None => Vec::new(),
    }
}
