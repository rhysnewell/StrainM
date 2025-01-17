use hashlink::{LinkedHashMap, LinkedHashSet};
use itertools::Itertools;
use ordered_float::OrderedFloat;
use std::process::Stdio;
use rust_htslib::bcf::header::HeaderView;
use rust_htslib::bcf::record::{GenotypeAllele, Numeric};
use rust_htslib::bcf::{IndexedReader, Read, Reader, Record, Writer};
use std::cmp::{min, Ordering};
use std::collections::HashSet;
use std::hash::{Hash, Hasher};
use std::ops::Range;
use std::path::Path;

use crate::genotype::genotype_builder::{
    AttributeObject, Genotype, GenotypeAssignmentMethod, GenotypesContext,
};
use crate::annotator::variant_annotation::VariantAnnotations;
use crate::external_command_checker::check_for_bcftools;
use crate::genotype::genotype_likelihood_calculators::GenotypeLikelihoodCalculators;
use crate::genotype::genotype_likelihoods::GenotypeLikelihoods;
use crate::genotype::genotype_prior_calculator::GenotypePriorCalculator;
use crate::model::byte_array_allele::{Allele, ByteArrayAllele};
use crate::model::variants::{Filter, NON_REF_ALLELE};
use crate::reference::reference_reader::ReferenceReader;
use crate::utils::math_utils::MathUtils;
use crate::utils::simple_interval::SimpleInterval;
use crate::utils::vcf_constants::*;

#[derive(Debug, Clone)]
pub struct VariantContext {
    pub loc: SimpleInterval,
    // variant alleles
    pub alleles: Vec<ByteArrayAllele>,
    // per sample likelihoods
    pub genotypes: GenotypesContext,
    pub source: String,
    pub log10_p_error: f64,
    pub filters: HashSet<Filter>,
    pub attributes: LinkedHashMap<String, AttributeObject>,
    pub variant_type: Option<VariantType>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum VariantType {
    NoVariation,
    Snp,
    Mnp,
    Indel,
    Symbolic,
    Mixed,
}

impl VariantType {
    pub fn to_key(&self) -> &str {
        match self {
            VariantType::NoVariation => ".",
            VariantType::Snp => "SNP",
            VariantType::Mnp => "MNP",
            VariantType::Indel => "INDEL",
            VariantType::Symbolic => "SYM",
            VariantType::Mixed => "MIXED",
        }
    }
}

// The priority queue depends on `Ord`.
// Explicitly implement the trait so the queue becomes a min-heap
// instead of a max-heap.
impl Ord for VariantContext {
    fn cmp(&self, other: &Self) -> Ordering {
        self.loc
            .tid
            .cmp(&other.loc.tid)
            .then_with(|| self.loc.start.cmp(&other.loc.start))
            .then_with(|| {
                self.get_reference()
                    .length()
                    .cmp(&other.get_reference().length())
            })
            .then_with(|| self.get_alternate_alleles()[0].cmp(&other.get_alternate_alleles()[0]))
    }
}

// `PartialOrd` needs to be implemented as well.
impl PartialOrd for VariantContext {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl Eq for VariantContext {}

impl PartialEq for VariantContext {
    fn eq(&self, other: &Self) -> bool {
        self.loc == other.loc && self.alleles == other.alleles
    }
}

impl Hash for VariantContext {
    fn hash<H: Hasher>(&self, state: &mut H) {
        self.loc.hash(state);
        self.alleles.hash(state);
        self.source.hash(state);
    }
}

impl VariantContext {
    pub const MAX_ALTERNATE_ALLELES: usize = 180;
    pub const SUM_GL_THRESH_NOCALL: f64 = -0.1; // if sum(gl) is bigger than this threshold, we treat GL's as non-informative and will force a no-call.

    pub fn empty(tid: usize, start: usize, end: usize) -> VariantContext {
        VariantContext {
            loc: SimpleInterval::new(tid, start, end),
            alleles: Vec::new(),
            genotypes: GenotypesContext::empty(),
            source: "".to_string(),
            log10_p_error: 0.0,
            filters: HashSet::new(),
            attributes: LinkedHashMap::new(),
            variant_type: None,
        }
    }

    pub fn build(
        tid: usize,
        start: usize,
        end: usize,
        alleles: Vec<ByteArrayAllele>,
    ) -> VariantContext {
        let alleles = Self::make_alleles(
            alleles
                .into_iter()
                .collect::<LinkedHashSet<ByteArrayAllele>>(),
        );
        VariantContext {
            loc: SimpleInterval::new(tid, start, end),
            alleles,
            genotypes: GenotypesContext::empty(),
            source: "".to_string(),
            log10_p_error: 0.0,
            filters: HashSet::new(),
            attributes: LinkedHashMap::new(),
            variant_type: None,
        }
    }

    pub fn build_from_vc(vc: &VariantContext) -> VariantContext {
        let alleles = Self::make_alleles(
            vc.alleles
                .iter()
                .cloned()
                .collect::<LinkedHashSet<ByteArrayAllele>>(),
        );
        VariantContext {
            loc: vc.loc.clone(),
            alleles,
            genotypes: vc.genotypes.clone(),
            source: vc.source.clone(),
            log10_p_error: vc.log10_p_error.clone(),
            filters: vc.filters.clone(),
            attributes: vc.attributes.clone(),
            variant_type: None,
        }
    }

    fn make_alleles(alleles: LinkedHashSet<ByteArrayAllele>) -> Vec<ByteArrayAllele> {
        let mut allele_list = Vec::new();

        let mut saw_ref = false;
        for a in alleles {
            if allele_list.contains(&a) {
                panic!("Duplicate allele added to VariantContext {:?}", &a)
            };

            // deal with the case where the first allele isn't the reference
            if a.is_reference() {
                if saw_ref {
                    panic!("Alleles for a VariantContext must contain at most one reference allele: {:?}", &a);
                }
                allele_list.insert(0, a);
                saw_ref = true;
            } else {
                allele_list.push(a);
            }
        }

        if allele_list.is_empty() {
            panic!("Cannot create a VariantContext with an empty allele list");
        }

        if !allele_list[0].is_reference() {
            panic!(
                "Alleles for a VariantContext must contain at least one reference allele: {:?}",
                &allele_list
            );
        }

        allele_list
    }

    pub fn is_filtered(&self) -> bool {
        !self.filters.is_empty()
    }

    pub fn is_not_filtered(&self) -> bool {
        !self.is_filtered()
    }

    /**
     * convenience method for variants
     *
     * @return true if this is a variant allele, false if it's reference
     */
    pub fn is_variant(&mut self) -> bool {
        self.get_type() != &VariantType::NoVariation
    }

    pub fn attributes(&mut self, attributes: LinkedHashMap<String, AttributeObject>) {
        self.attributes.extend(attributes);
    }

    pub fn set_attribute(&mut self, tag: String, value: AttributeObject) {
        self.attributes.insert(tag, value);
    }

    pub fn filter(&mut self, filter: Filter) {
        self.filters.insert(filter);
    }

    pub fn has_log10_p_error(&self) -> bool {
        (self.log10_p_error - 1.0).abs() > f64::EPSILON
    }

    pub fn get_log10_p_error(&self) -> f64 {
        self.log10_p_error
    }

    pub fn get_phred_scaled_qual(&self) -> f64 {
        (self.log10_p_error * (-10.0)) + 0.0
    }

    pub fn log10_p_error(&mut self, error: f64) {
        self.log10_p_error = error
    }

    pub fn add_alleles(&mut self, alleles: Vec<ByteArrayAllele>) {
        self.alleles = alleles
    }

    pub fn add_source(&mut self, source: String) {
        self.source = source
    }

    pub fn get_n_samples(&self) -> usize {
        self.genotypes.len()
    }

    pub fn get_n_alleles(&self) -> usize {
        self.alleles.len()
    }

    pub fn add_genotypes(&mut self, genotypes: Vec<Genotype>) {
        // for genotype in genotypes.iter() {
        //     if genotype.pl.len() != self.alleles.len() {
        //         panic!(
        //             "Number of likelihoods does not match number of alleles at position {} on tid {}",
        //             self.loc.get_start(), self.loc.get_contig()
        //         )
        //     }
        // }
        self.genotypes = GenotypesContext::new(genotypes);
    }

    pub fn has_non_ref_allele(&self) -> bool {
        self.alleles.iter().any(|a| !a.is_reference())
    }

    /**
     * Checks whether the variant context has too many alternative alleles for progress to genotyping the site.
     * <p>
     *     AF calculation may get into trouble with too many alternative alleles.
     * </p>
     *
     * @param vc the variant context to evaluate.
     *
     * @throws NullPointerException if {@code vc} is {@code null}.
     *
     * @return {@code true} iff there is too many alternative alleles based on
     * {@link GenotypeLikelihoods#MAX_DIPLOID_ALT_ALLELES_THAT_CAN_BE_GENOTYPED}.
     */
    pub fn has_too_many_alternative_alleles(&self) -> bool {
        if self.get_n_alleles()
            <= GenotypeLikelihoods::MAX_DIPLOID_ALT_ALLELES_THAT_CAN_BE_GENOTYPED
        {
            false
        } else {
            true
        }
    }

    /**
     * Add the genotype call (GT) field to GenotypeBuilder using the requested {@link GenotypeAssignmentMethod}
     *
     * @param gb the builder where we should put our newly called alleles, cannot be null
     * @param assignmentMethod the method to use to do the assignment, cannot be null
     * @param genotypeLikelihoods a vector of likelihoods to use if the method requires PLs, should be log10 likelihoods, cannot be null
     * @param allelesToUse the alleles with respect to which the likelihoods are defined
     */
    pub fn make_genotype_call(
        ploidy: usize,
        gb: &mut Genotype,
        assignment_method: &GenotypeAssignmentMethod,
        genotype_likelihoods: Option<Vec<f64>>,
        alleles_to_use: &Vec<ByteArrayAllele>,
        original_gt: &Vec<ByteArrayAllele>,
        gpc: &GenotypePriorCalculator,
    ) {
        match genotype_likelihoods {
            Some(genotype_likelihoods) => {
                match assignment_method {
                    &GenotypeAssignmentMethod::SetToNoCall => {
                        gb.no_call_alleles(ploidy);
                        gb.no_qg();
                    }
                    &GenotypeAssignmentMethod::UsePLsToAssign => {
                        if !VariantContext::is_informative(&genotype_likelihoods) {
                            gb.no_call_alleles(ploidy);
                            gb.no_qg();
                        } else {
                            let max_likelihood_index = MathUtils::max_element_index(
                                &genotype_likelihoods,
                                0,
                                genotype_likelihoods.len(),
                            );
                            let mut gl_calc = GenotypeLikelihoodCalculators::get_instance(
                                ploidy,
                                alleles_to_use.len(),
                            );
                            let allele_counts =
                                gl_calc.genotype_allele_counts_at(max_likelihood_index);
                            let final_alleles = allele_counts.as_allele_list(alleles_to_use);
                            if final_alleles.contains(&*NON_REF_ALLELE) {
                                gb.no_call_alleles(ploidy);
                                gb.pl = GenotypeLikelihoods::from_log10_likelihoods(
                                    vec![0.0; genotype_likelihoods.len()],
                                )
                                .as_pls();
                            } else {
                                gb.alleles = final_alleles;
                            }
                            let num_alt_alleles = alleles_to_use.len() - 1;
                            if num_alt_alleles > 0 {
                                gb.log10_p_error(
                                    GenotypeLikelihoods::get_gq_log10_from_likelihoods(
                                        max_likelihood_index,
                                        &genotype_likelihoods,
                                    ),
                                );
                            }
                        }
                    }
                    &GenotypeAssignmentMethod::SetToNoCallNoAnnotations => {
                        gb.no_call_alleles(ploidy);
                        gb.no_annotations();
                    }
                    &GenotypeAssignmentMethod::BestMatchToOriginal => {
                        let mut best = Vec::new();
                        let refr = &alleles_to_use[0];
                        for original_allele in original_gt.iter() {
                            if alleles_to_use.contains(original_allele)
                                || original_allele.is_no_call()
                            {
                                best.push(original_allele.clone())
                            } else {
                                best.push(refr.clone())
                            }
                        }
                        gb.alleles = best;
                    }
                    &GenotypeAssignmentMethod::UsePosteriorProbabilities => {
                        // Calculate posteriors.
                        let mut gl_calc = GenotypeLikelihoodCalculators::get_instance(
                            ploidy,
                            alleles_to_use.len(),
                        );
                        let log10_priors = gpc.get_log10_priors(&mut gl_calc, alleles_to_use);
                        let log10_posteriors =
                            MathUtils::ebe_add(&log10_priors, &genotype_likelihoods);
                        let normalized_log10_posteriors =
                            MathUtils::scale_log_space_array_for_numeric_stability(
                                &log10_posteriors,
                            );
                        // Update GP and PG annotations:
                        gb.attribute(
                            GENOTYPE_POSTERIORS_KEY.to_string(),
                            AttributeObject::Vecf64(
                                normalized_log10_posteriors
                                    .iter()
                                    .map(|v| {
                                        if *v == 0.0 {
                                            // the reason for the == 0.0 is to avoid a signed 0 output "-0.0"
                                            0.0
                                        } else {
                                            v * -10.
                                        }
                                    })
                                    .collect::<Vec<f64>>(),
                            ),
                        );

                        gb.attribute(
                            GENOTYPE_PRIOR_KEY.to_string(),
                            AttributeObject::Vecf64(
                                log10_priors
                                    .iter()
                                    .map(|v| if *v == 0.0 { 0.0 } else { v * -10. })
                                    .collect::<Vec<f64>>(),
                            ),
                        );
                        // Set the GQ accordingly
                        let max_posterior_index = MathUtils::max_element_index(
                            &log10_posteriors,
                            0,
                            log10_posteriors.len(),
                        );
                        if alleles_to_use.len() > 0 {
                            gb.log10_p_error(VariantContext::get_gq_log10_from_posteriors(
                                max_posterior_index,
                                &normalized_log10_posteriors,
                            ));
                        }
                        // Finally we update the genotype alleles.
                        gb.alleles(
                            gl_calc
                                .genotype_allele_counts_at(max_posterior_index)
                                .as_allele_list(alleles_to_use),
                        );
                    }
                    &GenotypeAssignmentMethod::DoNotAssignGenotypes => {
                        // pass
                    }
                }
            }
            _ => {
                gb.no_call_alleles(ploidy);
                gb.no_qg();
            }
        }
    }

    /// Returns and owned representation of the consensus allele at this position,
    /// that is the allele with highest sequencing depth in the specified sample index.
    pub fn get_consensus_allele(&self, sample_index: usize) -> Option<ByteArrayAllele> {
        let mut current_max_depth = std::i32::MIN;
        let mut current_consensus = None;
        for (i, dp) in self.genotypes.genotypes()[sample_index]
            .ad
            .iter()
            .enumerate()
        {
            if dp > &current_max_depth {
                current_max_depth = *dp;
                current_consensus = Some(self.alleles[i].clone());
            }
        }

        // debug!(
        //     "Max Depth {} All depths {:?} Consensus {:?} genotypes {:?}",
        //     current_max_depth,
        //     self.genotypes.genotypes()[sample_index],
        //     &current_consensus,
        //     self.genotypes.genotypes()
        // );

        if current_max_depth == 0 {
            // no variant was found in this sample
            return None;
        }

        current_consensus
    }

    /// Returns index of the consensus allele at this position,
    /// that is the allele with highest sequencing depth in the specified sample index.
    pub fn get_consensus_allele_index(&self, sample_index: usize) -> Option<usize> {
        let mut current_max_depth = std::i32::MIN;
        let mut current_consensus = None;
        for (i, dp) in self.genotypes.genotypes()[sample_index]
            .ad
            .iter()
            .enumerate()
        {
            if dp > &current_max_depth {
                current_max_depth = *dp;
                current_consensus = Some(i);
            }
        }

        // debug!(
        //     "Max Depth {} All depths {:?} Consensus {:?} genotypes {:?}",
        //     current_max_depth,
        //     self.genotypes.genotypes()[sample_index],
        //     &current_consensus,
        //     self.genotypes.genotypes()
        // );

        if current_max_depth == 0 {
            // no variant was found in this sample
            return None;
        }

        current_consensus
    }

    /// Detects if alleles are present in a sample given sample index and the read coverage threshold
    pub fn alleles_present_in_sample(&self, sample_index: usize, threshold: i32) -> Vec<bool> {
        (0..self.get_n_alleles())
            .map(|allele_index| {
                self.genotypes.genotypes()[sample_index].ad[allele_index] >= threshold
            })
            .collect::<Vec<bool>>()
    }

    fn get_gq_log10_from_posteriors(best_genotype_index: usize, log10_posteriors: &[f64]) -> f64 {
        match log10_posteriors.len() {
            0 | 1 => 1.0,
            2 => {
                if best_genotype_index == 0 {
                    log10_posteriors[1]
                } else {
                    log10_posteriors[0]
                }
            }
            3 => std::cmp::min(
                OrderedFloat(0.0),
                OrderedFloat(MathUtils::log10_sum_log10_two_values(
                    log10_posteriors[if best_genotype_index == 0 {
                        2
                    } else {
                        best_genotype_index - 1
                    }],
                    log10_posteriors[if best_genotype_index == 2 {
                        0
                    } else {
                        best_genotype_index + 1
                    }],
                )),
            )
            .into_inner(),
            _ => {
                if best_genotype_index == 0 {
                    MathUtils::log10_sum_log10(log10_posteriors, 1, log10_posteriors.len())
                } else if best_genotype_index == (log10_posteriors.len() - 1) {
                    MathUtils::log10_sum_log10(log10_posteriors, 0, best_genotype_index)
                } else {
                    std::cmp::min(
                        OrderedFloat(0.0),
                        OrderedFloat(MathUtils::log10_sum_log10_two_values(
                            MathUtils::log10_sum_log10(log10_posteriors, 0, best_genotype_index),
                            MathUtils::log10_sum_log10(
                                log10_posteriors,
                                best_genotype_index + 1,
                                log10_posteriors.len(),
                            ),
                        )),
                    )
                    .into_inner()
                }
            }
        }
    }

    pub fn is_informative(gls: &[f64]) -> bool {
        gls.iter().sum::<f64>() < VariantContext::SUM_GL_THRESH_NOCALL
    }

    /**
     * Subset the samples in VC to reference only information with ref call alleles
     *
     * Preserves DP if present
     *
     * @param vc the variant context to subset down to
     * @param defaultPloidy defaultPloidy to use if a genotype doesn't have any alleles
     * @return a GenotypesContext
     */
    pub fn subset_to_ref_only(vc: &VariantContext, default_ploidy: usize) -> GenotypesContext {
        if default_ploidy < 1 {
            panic!("default_ploidy must be >= 1, got {}", default_ploidy)
        } else {
            let old_gts = vc.get_genotypes().clone();
            if old_gts.is_empty() {
                return old_gts;
            }

            let mut new_gts = GenotypesContext::create(old_gts.size());

            let ref_allele = vc.get_reference();
            let diploid_ref_alleles = vec![ref_allele.clone(); 2];

            // create the new genotypes
            for g in old_gts.genotypes() {
                let g_ploidy = if g.get_ploidy() == 0 {
                    default_ploidy
                } else {
                    g.get_ploidy()
                };
                let ref_alleles = if g_ploidy == 2 {
                    diploid_ref_alleles.clone()
                } else {
                    vec![ref_allele.clone(); g_ploidy]
                };

                let genotype = Genotype::build_from_alleles(ref_alleles, 0);
                new_gts.add(genotype);
            }

            return new_gts;
        }
    }

    pub fn get_attribute_as_int(&self, key: &String, default_value: usize) -> usize {
        match self.attributes.get(key) {
            Some(value) => {
                match value {
                    AttributeObject::UnsizedInteger(value) => return *value,
                    _ => return default_value, // value can not sensibly be converted to usize
                }
            }
            None => return default_value,
        }
    }

    pub fn get_genotypes(&self) -> &GenotypesContext {
        &self.genotypes
    }

    pub fn get_genotypes_mut(&mut self) -> &mut GenotypesContext {
        &mut self.genotypes
    }

    pub fn get_alleles(&self) -> &Vec<ByteArrayAllele> {
        &self.alleles
    }

    pub fn get_alleles_ref(&self) -> Vec<&ByteArrayAllele> {
        self.alleles.iter().collect::<Vec<&ByteArrayAllele>>()
    }

    pub fn get_reference(&self) -> &ByteArrayAllele {
        self.alleles.iter().find(|a| a.is_ref).unwrap()
    }

    pub fn get_reference_and_index(&self) -> (usize, &ByteArrayAllele) {
        self.alleles
            .iter()
            .enumerate()
            .find(|(_, a)| a.is_ref)
            .unwrap()
    }

    pub fn get_alleles_with_index(&self) -> Vec<(usize, &ByteArrayAllele)> {
        self.alleles.iter().enumerate().collect()
    }

    pub fn get_dp(&self) -> i32 {
        self.genotypes.get_dp()
    }

    pub fn get_alternate_alleles(&self) -> Vec<&ByteArrayAllele> {
        self.alleles.iter().filter(|a| !a.is_ref).collect()
    }

    pub fn get_alternate_alleles_with_index(&self) -> Vec<(usize, &ByteArrayAllele)> {
        self.alleles
            .iter()
            .enumerate()
            .filter(|(_, a)| !a.is_ref)
            .collect()
    }

    pub fn process_vcf_in_region(
        indexed_vcf: &mut IndexedReader,
        tid: u32,
        start: u64,
        end: u64,
    ) -> Vec<VariantContext> {
        indexed_vcf.fetch(tid, start, Some(end)).expect("Failed to fetch region");

        let variant_contexts = indexed_vcf
            .records()
            .into_iter()
            .map(|record| {
                let mut vcf_record = record.unwrap();
                let vc = Self::from_vcf_record(&mut vcf_record, true);
                vc.unwrap()
            })
            .collect::<Vec<VariantContext>>();

        return variant_contexts;
    }

    pub fn process_vcf_from_path(vcf_path: &str, with_depth: bool) -> Vec<VariantContext> {
        let mut vcf_reader = Reader::from_path(vcf_path);
        match vcf_reader {
            Ok(ref mut reader) => {
                let variant_contexts = reader
                    .records()
                    .into_iter()
                    .map(|record| {
                        let mut vcf_record = record.unwrap();
                        let vc = Self::from_vcf_record(&mut vcf_record, with_depth);
                        vc.unwrap()
                    })
                    .collect::<Vec<VariantContext>>();

                return variant_contexts;
            }
            Err(_) => {
                // debug!("No VCF records found for {}", vcf_path);
                return Vec::new();
            }
        }
    }

    pub fn retrieve_indexed_vcf_file(file: &str) -> IndexedReader {
        match IndexedReader::from_path(file) {
            Ok(vcf_reader) => {
                // debug!("Found readable VCF file");
                return vcf_reader;
            }
            Err(_e) => {
                // vcf file likely not indexed
                Self::generate_vcf_index(file)
            }
        }
    }

    pub fn get_vcf_reader<S: AsRef<str>>(vcf_path: S) -> IndexedReader {
        match IndexedReader::from_path(format!("{}.gz", vcf_path.as_ref())) {
            Ok(vcf_reader) => {
                // debug!("Found readable VCF file");
                return vcf_reader;
            }
            Err(_e) => {
                // vcf file likely not indexed
                Self::generate_vcf_index(vcf_path)
            }
        }
    }

    pub fn generate_vcf_index<S: AsRef<str>>(vcf_path: S) -> IndexedReader {
        check_for_bcftools();
        // debug!("Generating VCF index");
        let gzip_path = format!("{}.gz", vcf_path.as_ref());
        let cmd_string = if !Path::new(&gzip_path).exists() {
            format!(
                "set -e -o pipefail; \
            bgzip {}; bcftools index {}.gz",
                vcf_path.as_ref(),
                vcf_path.as_ref(),
            )
        } else {
            format!(
                "set -e -o pipefail; \
                bcftools index {}.gz",
                vcf_path.as_ref(),
            )
        };

        // debug!("Queuing cmd_string: {}", cmd_string);

        std::process::Command::new("bash")
            .arg("-c")
            .arg(&cmd_string)
            .stdout(Stdio::piped())
            .output()
            .expect("Unable to execute bash");

        return IndexedReader::from_path(&gzip_path).expect(
            "Unable to generate index. Please ensure you have gzipped your file using 'bgzip -c'.",
        );
    }

    pub fn from_vcf_record(record: &mut Record, with_depths: bool) -> Option<VariantContext> {
        // debug!("Found VCF record with {:?} alleles", record.allele_count());
        let variants = Self::collect_variants(record, false, false, None);
        if variants.len() == 0 {
            return None;
        }

        // Get elements from record
        let mut vc = Self::build(
            record.rid().unwrap() as usize,
            record.pos() as usize,
            record.pos() as usize,
            variants,
        );
        if with_depths {
            let allele_depths = record.format(b"AD").integer().unwrap();
            let genotype_tags = record.format(b"GT").string().unwrap();
            let ploidy = genotype_tags
                .as_slice()
                .iter()
                .map(|g| g.len())
                .max()
                .unwrap();
            // debug!(
            //     "Allele depths {:?} {:?}",
            //     &allele_depths,
            //     record.format(b"AD").integer().unwrap()
            // );
            let genotypes = allele_depths
                .iter()
                .map(|depths| {
                    let mut depths = depths.into_iter().map(|d| *d as i32).collect::<Vec<i32>>();
                    if depths.len() == 1 {
                        depths = vec![0; vc.alleles.len()];
                    };
                    // println!("Depths {:?}", &depths);
                    Genotype::build_from_ads(ploidy, depths)
                })
                .collect::<Vec<Genotype>>();

            let genotype_context = GenotypesContext::new(genotypes);
            vc.genotypes = genotype_context;

            let qd = record.info(b"QD").float().unwrap();
            vc.attributes.insert(
                VariantAnnotations::QualByDepth.to_key().to_string(),
                AttributeObject::f64(qd.unwrap()[0] as f64),
            );
        }
        vc.log10_p_error((record.qual() as f64) / -10.0);

        // Separate filters into hashset of filter struct
        let filters = record.filters();
        let header = record.header();
        for filter in filters {
            vc.filter(Filter::from_result(std::str::from_utf8(
                &header.id_to_name(filter)[..],
            )));
        }
        // debug!("VC {:?}", &vc);
        Some(vc)
    }

    /// Collect variants from a given ´bcf::Record`.
    pub fn collect_variants(
        record: &mut Record,
        omit_snvs: bool,
        _omit_indels: bool,
        _indel_len_range: Option<Range<u32>>,
    ) -> Vec<ByteArrayAllele> {
        let _pos = record.pos();
        let _svlens = match record.info(b"SVLEN").integer() {
            // Gets value from SVLEN tag in VCF record
            Ok(Some(svlens)) => Some(
                svlens
                    .into_iter()
                    .map(|l| {
                        if !l.is_missing() {
                            Some(l.abs() as u32)
                        } else {
                            None
                        }
                    })
                    .collect_vec(),
            ),
            _ => None,
        };
        let _end = match record.info(b"END").integer() {
            Ok(Some(end)) => {
                let end = end[0] as u32 - 1;
                Some(end)
            }
            _ => None,
        };

        // check if len is within the given range
        // let is_valid_len = |svlen| {
        //     if let Some(ref len_range) = indel_len_range {
        //         // TODO replace with Range::contains once stabilized
        //         if svlen < len_range.start || svlen >= len_range.end {
        //             return false;
        //         }
        //     }
        //     true
        // };

        let _is_valid_insertion_alleles = |ref_allele: &[u8], alt_allele: &[u8]| {
            alt_allele == b"<INS>"
                || (ref_allele.len() < alt_allele.len()
                    && ref_allele == &alt_allele[..ref_allele.len()])
        };

        let _is_valid_deletion_alleles = |ref_allele: &[u8], alt_allele: &[u8]| {
            alt_allele == b"<DEL>"
                || (ref_allele.len() > alt_allele.len()
                    && &ref_allele[..alt_allele.len()] == alt_allele)
        };

        let _is_valid_inversion_alleles = |ref_allele: &[u8], alt_allele: &[u8]| {
            alt_allele == b"<INV>" || (ref_allele.len() == alt_allele.len())
        };

        let _is_valid_mnv = |ref_allele: &[u8], alt_allele: &[u8]| {
            alt_allele == b"<MNV>" || (ref_allele.len() == alt_allele.len())
        };

        let variants = {
            let alleles = record.alleles();
            let ref_allele = alleles[0];
            let mut variant_vec = vec![];
            alleles.iter().enumerate().for_each(|(i, alt_allele)| {
                let is_reference = i == 0;
                // if alt_allele == b"*" {
                //     // dummy non-ref allele, signifying potential homozygous reference site
                //     if omit_snvs {
                //         variant_vec.push(ByteArrayAllele::new(".".as_bytes(), is_reference))
                //     } else {
                //         variant_vec.push(ByteArrayAllele::new(".".as_bytes(), is_reference))
                //     }
                // } else
                if alt_allele == b"<DEL>" {
                    // if let Some(ref svlens) = svlens {
                    //     if let Some(svlen) = svlens[i] {
                    //         variant_vec.push(ByteArrayAllele::new("*".as_bytes(), is_reference))
                    //     } else {
                    //         // TODO fail with an error in this case
                    //         variant_vec.push(ByteArrayAllele::new(".".as_bytes(), is_reference))
                    //     }
                    // } else {
                    //     // TODO fail with an error in this case
                    //     variant_vec.push(ByteArrayAllele::new(".".as_bytes(), is_reference))
                    // }
                } else if alt_allele[0] == b'<' {
                    // TODO Catch <DUP> structural variants here
                    // skip any other special alleles
                    // variant_vec.push(ByteArrayAllele::new(".".as_bytes(), is_reference))
                } else if alt_allele.len() == 1 && ref_allele.len() == 1 {
                    // SNV
                    if omit_snvs {
                        variant_vec.push(ByteArrayAllele::new(".".as_bytes(), is_reference))
                    } else {
                        // debug!("Reading in SNV {:?} {:?}", ref_allele, alt_allele);
                        variant_vec.push(ByteArrayAllele::new(alt_allele, is_reference))
                    }
                } else {
                    let _indel_len =
                        (alt_allele.len() as i32 - ref_allele.len() as i32).abs() as u32;
                    // TODO fix position if variant is like this: cttt -> ct
                    variant_vec.push(ByteArrayAllele::new(alt_allele, is_reference))
                    // if (omit_indels || !is_valid_len(indel_len))
                    //     && is_valid_mnv(ref_allele, alt_allele)
                    // {
                    //     // debug!("Reading in MNV {:?} {:?}", ref_allele, alt_allele);
                    //     variant_vec.push(ByteArrayAllele::new(alt_allele, is_reference))
                    // } else if is_valid_deletion_alleles(ref_allele, alt_allele) {
                    //     variant_vec.push(ByteArrayAllele::new("*".as_bytes(), is_reference))
                    // } else if is_valid_insertion_alleles(ref_allele, alt_allele) {
                    //     variant_vec.push(ByteArrayAllele::new(
                    //         &alt_allele[ref_allele.len()..],
                    //         is_reference,
                    //     ))
                    // } else if is_valid_mnv(ref_allele, alt_allele) {
                    //     // println!("MNV 2 {:?} {:?}", ref_allele, alt_allele);
                    //     variant_vec.push(ByteArrayAllele::new(alt_allele, is_reference))
                    // }
                }
            });
            variant_vec
        };

        variants
    }

    /// Attempts to retrieve a VCF rid given a contig name
    /// If the provided contig name contains a `~` symbol, then this function attempts to remove
    /// and find the rid from a split in the contig name
    pub fn get_contig_vcf_tid(vcf_header: &HeaderView, contig_name: &[u8]) -> Option<u32> {
        match vcf_header.name2rid(contig_name) {
            Ok(rid) => Some(rid),
            Err(_) => {
                // Remove leading reference stem
                // debug!(
                //     "Attempting to find {:?}",
                //     std::str::from_utf8(ReferenceReader::split_contig_name(contig_name, '~' as u8))
                // );
                match vcf_header
                    .name2rid(ReferenceReader::split_contig_name(contig_name, '~' as u8))
                {
                    Ok(rid) => Some(rid),
                    Err(_) => match vcf_header.name2rid(contig_name) {
                        Ok(rid) => Some(rid),
                        Err(_) => None,
                    },
                }
            }
        }
    }

    pub fn is_snp(&mut self) -> bool {
        return self.get_type() == &VariantType::Snp;
    }

    pub fn is_indel(&mut self) -> bool {
        return self.get_type() == &VariantType::Indel;
    }

    pub fn get_type(&mut self) -> &VariantType {
        if self.variant_type.is_none() {
            self.determine_type()
        };

        return self.variant_type.as_ref().unwrap();
    }

    pub fn set_type(&mut self, variant_type: VariantType) {
        self.variant_type = Some(variant_type);
    }

    pub fn determine_type(&mut self) {
        if self.variant_type.is_none() {
            match self.alleles.len() {
                0 => {
                    panic!("Unexpected error: requested type of VariantContext with no alleles")
                }
                1 => {
                    // note that this doesn't require a reference allele.  You can be monomorphic independent of having a
                    // reference allele
                    self.variant_type = Some(VariantType::NoVariation);
                }
                _ => self.determine_polymorphic_type(),
            }
        }
    }

    pub fn determine_polymorphic_type(&mut self) {
        // do a pairwise comparison of all alleles against the reference allele
        for allele in self.alleles.iter() {
            if allele.is_ref {
                continue;
            }

            // find the type of this allele relative to the reference
            let biallelic_type = Self::type_of_biallelic_variant(self.get_reference(), allele);

            if self.variant_type.is_none() {
                self.variant_type = Some(biallelic_type);
            } else if &biallelic_type != self.variant_type.as_ref().unwrap() {
                self.variant_type = Some(VariantType::Mixed);
                break;
            }
        }
    }

    pub fn type_of_biallelic_variant(
        reference: &ByteArrayAllele,
        allele: &ByteArrayAllele,
    ) -> VariantType {
        if reference.is_symbolic {
            panic!("Unexpected error: Encountered a record with a symbolic reference allele")
        };

        if allele.is_symbolic {
            return VariantType::Symbolic;
        }

        if reference.len() == allele.len() {
            if allele.len() == 1 {
                return VariantType::Snp;
            } else {
                return VariantType::Mnp;
            }
        }

        // Important note: previously we were checking that one allele is the prefix of the other.  However, that's not an
        // appropriate check as can be seen from the following example:
        // REF = CTTA and ALT = C,CT,CA
        // This should be assigned the INDEL type but was being marked as a MIXED type because of the prefix check.
        // In truth, it should be absolutely impossible to return a MIXED type from this method because it simply
        // performs a pairwise comparison of a single alternate allele against the reference allele (whereas the MIXED type
        // is reserved for cases of multiple alternate alleles of different types).  Therefore, if we've reached this point
        // in the code (so we're not a SNP, MNP, or symbolic allele), we absolutely must be an INDEL.
        return VariantType::Indel;
    }

    pub fn get_called_chr_count(&self) -> i32 {
        let mut n = 0;
        let genotypes = self.get_genotypes();

        for g in genotypes.genotypes() {
            for a in g.alleles.iter() {
                if !a.is_no_call() {
                    n += 1;
                }
            }
        }

        n
    }

    pub fn get_called_chr_count_from_allele(&self, a: &ByteArrayAllele) -> i32 {
        let mut n = 0;
        let genotypes = self.get_genotypes();

        for g in genotypes.genotypes() {
            n += g.count_allele(a);
        }

        n
    }

    /**
     * @return true if the alleles indicate a simple insertion (i.e., the reference allele is Null)
     */
    pub fn is_simple_insertion(&mut self) -> bool {
        // can't just call !isSimpleDeletion() because of complex indels
        return self.is_simple_indel() && self.get_reference().length() == 1;
    }

    /**
     * @return true if the alleles indicate a simple deletion (i.e., a single alt allele that is Null)
     */
    pub fn is_simple_deletion(&mut self) -> bool {
        // can't just call !isSimpleInsertion() because of complex indels
        return self.is_simple_indel() && self.get_alternate_alleles()[0].length() == 1;
    }

    /**
     * convenience method for indels
     *
     * @return true if this is an indel, false otherwise
     */
    pub fn is_simple_indel(&mut self) -> bool {
        return self.is_indel() && // allelic lengths differ
            self.is_biallelic() && // exactly 2 alleles
            self.get_reference().len() > 0 && // ref is not null or symbolic
            self.get_alternate_alleles()[0].len() > 0 && // alt is not null or symbolic
            self.get_reference().get_bases()[0] == self.get_alternate_alleles()[0].get_bases()[0] && // leading bases match for both alleles
            (self.get_reference().len() == 1 || self.get_alternate_alleles()[0].len() == 1);
    }

    /**
     * @return true if the context is strictly bi-allelic
     */
    pub fn is_biallelic(&self) -> bool {
        self.alleles.len() == 2
    }

    /**
     * Returns the maximum ploidy of all samples in this VC, or default if there are no genotypes
     *
     * This function is caching, so it's only expensive on the first call
     *
     * @param defaultPloidy the default ploidy, if all samples are no-called
     * @return default, or the max ploidy
     */
    pub fn get_max_ploidy(&mut self, default_ploidy: usize) -> i32 {
        self.genotypes.get_max_ploidy(default_ploidy)
    }

    pub fn get_alleles_as_bytes(&self) -> Vec<&[u8]> {
        self.get_alleles()
            .into_iter()
            .map(|a| a.get_bases())
            .collect::<Vec<&[u8]>>()
    }

    /// Returns a boolean indicating whether this `VariantContext` is part of the given strain
    pub fn part_of_strain(&self, strain_id: usize) -> bool {
        match self.attributes.get(VariantAnnotations::Strain.to_key()) {
            None => {
                // Strain annotation not present so panic
                false
            }
            Some(attribute) => {
                if let AttributeObject::VecUnsize(vec) = attribute {
                    vec.contains(&strain_id)
                } else {
                    false
                }
            }
        }
    }

    /// writes this VariantContext as a VCF4 record. Assumes writer has prepopulated all INFO
    /// and FORMAT fields using the variant annotation engine.
    pub fn write_as_vcf_record(
        &self,
        bcf_writer: &mut Writer,
        reference_reader: &ReferenceReader,
        n_samples: usize,
    ) {
        let mut record = bcf_writer.empty_record();
        let rid = bcf_writer
            .header()
            .name2rid(reference_reader.get_target_name(self.loc.get_contig()))
            .expect("Contig name not present in BCF header");
        record.set_rid(Some(rid));
        record.set_pos(self.loc.start as i64); // 0-based
        record.set_qual(-10.0 * self.log10_p_error as f32);
        match &self.variant_type {
            None => {
                record.set_id(b".").expect("Failed to set id");
            }
            Some(variant_type) => {
                record
                    .set_id(variant_type.to_key().as_bytes())
                    .expect("Failed to set id");
            }
        }
        record
            .set_alleles(&self.get_alleles_as_bytes())
            .expect("Failed to set alleles");
        if !self.filters.is_empty() {
            for filter in &self.filters {
                if filter.to_key().as_bytes() != b"." {
                    record
                        .push_filter(filter.to_key().as_bytes())
                        .expect("Failed to set filter");
                }
            }
        }

        self.add_genotype_format(&mut record, n_samples);

        self.add_variant_info(&mut record);

        bcf_writer.write(&record).unwrap();
    }

    /// Removes all other alts attribtues except the one provided by alt_index
    /// Also keeps the reference
    pub fn remove_attributes_for_alt_by_index(&mut self, alt_index: usize) {
        if self
            .attributes
            .contains_key(VariantAnnotations::MappingQuality.to_key())
        {
            if let AttributeObject::VecU8(val) = self
                .attributes
                .get_mut(VariantAnnotations::MappingQuality.to_key())
                .unwrap()
            {
                if alt_index < val.len() {
                    *val = vec![val[0], val[alt_index]];
                }
            }
        }

        if self
            .attributes
            .contains_key(VariantAnnotations::BaseQuality.to_key())
        {
            if let AttributeObject::VecU8(val) = self
                .attributes
                .get_mut(VariantAnnotations::BaseQuality.to_key())
                .unwrap()
            {
                if alt_index < val.len() {
                    *val = vec![val[0], val[alt_index]];
                }
            }
        }
    }

    fn add_variant_info(&self, record: &mut Record) {
        if self
            .attributes
            .contains_key(VariantAnnotations::QualByDepth.to_key())
        {
            if let AttributeObject::f64(val) = self
                .attributes
                .get(VariantAnnotations::QualByDepth.to_key())
                .unwrap()
            {
                record
                    .push_info_float(
                        VariantAnnotations::QualByDepth.to_key().as_bytes(),
                        &[*val as f32],
                    )
                    .expect("Cannot push info tag");
            }
        }

        if self
            .attributes
            .contains_key(VariantAnnotations::Depth.to_key())
        {
            if let AttributeObject::UnsizedInteger(val) = self
                .attributes
                .get(VariantAnnotations::Depth.to_key())
                .unwrap()
            {
                record
                    .push_info_integer(
                        VariantAnnotations::Depth.to_key().as_bytes(),
                        &vec![*val as i32],
                    )
                    .expect("Cannot push info tag");
            }
        }

        if self
            .attributes
            .contains_key(VariantAnnotations::AlleleFraction.to_key())
        {
            if let AttributeObject::Vecf64(val) = self
                .attributes
                .get(VariantAnnotations::AlleleFraction.to_key())
                .unwrap()
            {
                let val = val.into_iter().map(|v| *v as f32).collect::<Vec<f32>>();
                record
                    .push_info_float(
                        VariantAnnotations::AlleleFraction.to_key().as_bytes(),
                        val.as_slice(),
                    )
                    .expect("Cannot push info tag");
            }
        }

        if self
            .attributes
            .contains_key(VariantAnnotations::MappingQuality.to_key())
        {
            if let AttributeObject::VecU8(val) = self
                .attributes
                .get(VariantAnnotations::MappingQuality.to_key())
                .unwrap()
            {
                let val = val.into_iter().map(|v| *v as i32).collect::<Vec<i32>>();
                record
                    .push_info_integer(VariantAnnotations::MappingQuality.to_key().as_bytes(), &val)
                    .expect("Cannot push info tag");
            }
        }

        if self
            .attributes
            .contains_key(VariantAnnotations::BaseQuality.to_key())
        {
            if let AttributeObject::VecU8(val) = self
                .attributes
                .get(VariantAnnotations::BaseQuality.to_key())
                .unwrap()
            {
                let val = val.into_iter().map(|v| *v as i32).collect::<Vec<i32>>();
                record
                    .push_info_integer(VariantAnnotations::BaseQuality.to_key().as_bytes(), &val)
                    .expect("Cannot push info tag");
            }
        }

        if self
            .attributes
            .contains_key(VariantAnnotations::VariantGroup.to_key())
        {
            if let AttributeObject::I32(val) = self
                .attributes
                .get(VariantAnnotations::VariantGroup.to_key())
                .unwrap()
            {
                record
                    .push_info_integer(
                        VariantAnnotations::VariantGroup.to_key().as_bytes(),
                        &[*val],
                    )
                    .expect("Cannot push info tag");
            }
        }

        if self
            .attributes
            .contains_key(VariantAnnotations::Strain.to_key())
        {
            if let AttributeObject::VecUnsize(val) = self
                .attributes
                .get(VariantAnnotations::Strain.to_key())
                .unwrap()
            {
                let val = val.into_iter().map(|v| *v as i32).collect::<Vec<i32>>();
                record
                    .push_info_integer(VariantAnnotations::Strain.to_key().as_bytes(), &val)
                    .expect("Cannot push info tag");
            }
        }

        if self
            .attributes
            .contains_key(VariantAnnotations::Qualified.to_key())
        {
            if let AttributeObject::String(val) = self
                .attributes
                .get(VariantAnnotations::Qualified.to_key())
                .unwrap()
            {
                let val = val.as_bytes();
                record
                    .push_info_string(VariantAnnotations::Qualified.to_key().as_bytes(), &[val])
                    .expect("Cannot push info tag");
            }
        }
    }

    fn add_genotype_format(&self, record: &mut Record, _n_samples: usize) {
        // let mut genotype_alleles = Vec::with_capacity(self.genotypes.len());
        let mut phases = Vec::new();
        let mut pls = Vec::new();
        let mut ads = Vec::new();
        let mut gqs = Vec::new();
        let mut dps = Vec::new();
        for genotype in self.genotypes.genotypes() {
            if genotype.dp == -1 || genotype.dp == 0 || genotype.alleles.len() == 0 {
                phases.extend(vec![GenotypeAllele::UnphasedMissing; genotype.ploidy]);
                pls.push(genotype.pl_str());
                ads.push(genotype.ad_str());
                dps.push(0);
                gqs.push(0);
                continue;
            };

            let mut phased = vec![GenotypeAllele::Unphased(0); genotype.ploidy];
            // let n_alleles = genotype.alleles.len();
            let pls_index = genotype
                .pl
                .iter()
                .enumerate()
                .min_by(|(_, a), (_, b)| a.cmp(b))
                .map(|(index, _)| index)
                .unwrap();

            let genotype_tag_vals =
                Self::calculate_genotype_tag(pls_index, genotype.ploidy, genotype.alleles.len());

            genotype_tag_vals
                .into_iter()
                .enumerate()
                .for_each(|(i, tag_val)| {
                    if genotype.is_phased && i != 0 {
                        phased[i] = GenotypeAllele::Phased(tag_val)
                    } else {
                        phased[i] = GenotypeAllele::Unphased(tag_val)
                    }
                });

            phases.extend(phased);

            pls.push(genotype.pl_str());
            ads.push(genotype.ad_str());
            dps.push(genotype.dp as i32);
            gqs.push(genotype.gq as i32);
        }

        record
            .push_genotypes(phases.as_slice())
            .expect("Unable to push genotypes");

        record
            .push_format_string(
                VariantAnnotations::PhredLikelihoods.to_key().as_bytes(),
                &pls.iter().map(|p| p.as_bytes()).collect::<Vec<&[u8]>>(),
            )
            .expect("Unable to push format tag");

        record
            .push_format_string(
                VariantAnnotations::DepthPerAlleleBySample
                    .to_key()
                    .as_bytes(),
                &ads.iter().map(|a| a.as_bytes()).collect::<Vec<&[u8]>>(),
            )
            .expect("Unable to push format tag");
        record
            .push_format_integer(
                VariantAnnotations::GenotypeQuality.to_key().as_bytes(),
                &gqs,
            )
            .expect("Unable to push format tag");
        record
            .push_format_integer(VariantAnnotations::Depth.to_key().as_bytes(), &dps)
            .expect("Unable to push format tag");
    }

    /// Given the most likely index from a set of likelihoods i.e. for phred scaled [10, 0, 20],
    /// index 1 would be the most likely. Also, given the ploidy and number of alleles in this
    /// genotype, calculate the genotype tag.
    ///
    /// For example pls [10, 0, 20], pls_index=1, ploidy=2, n_alleles=2 then return [0, 1] indicating
    /// this individual is heterozygous.
    ///
    /// For three alleles [10, 20, 30, 40, 0, 60], the number of likelihoods increases.
    /// this would return [1, 2], indicating the individual is heterozygous with two alternative alleles
    ///
    /// Should be stable over all ploidies and allele counts
    pub fn calculate_genotype_tag(pls_index: usize, ploidy: usize, n_alleles: usize) -> Vec<i32> {
        let mut genotype_tag_vals = vec![0; ploidy];
        if pls_index == 0 {
            return genotype_tag_vals; // easy homozygous ref result
        }
        let mut ploidy_index = ploidy.saturating_sub(1); // start from back of tag

        let mut allele_counts = vec![0; ploidy]; // count of times we have saturated this position
        let _allele_index = ploidy.saturating_sub(1); // this is the allele index of the last tag
        // let mut rounds = 0;
        let mut max_allele_allowed = 1;
        for _tag_index in 0..pls_index {
            if ploidy_index == 0 && genotype_tag_vals[ploidy_index] == max_allele_allowed {
                // reached end of round
                // rounds += 1;
                ploidy_index = ploidy.saturating_sub(1); // reset ploidy index
                genotype_tag_vals.fill(0);

                // as more alleles appear, more tags appear. These tags fill up (to a max value of
                // n_allele - 1), once a tag is full we need to use the number of rounds to determine
                // if the next tag is also full
                genotype_tag_vals[ploidy_index] = max_allele_allowed;
                max_allele_allowed = min(max_allele_allowed + 1, n_alleles as i32 - 1);
            }

            if genotype_tag_vals[ploidy_index] == max_allele_allowed {
                ploidy_index = ploidy_index.saturating_sub(1);
                genotype_tag_vals[ploidy_index] += 1;
            } else {
                allele_counts[ploidy_index] += 1;
                genotype_tag_vals[ploidy_index] += 1;
            }
        }

        genotype_tag_vals
    }

    // /// Calculates Fst, or Fixation index, for two or more samples at a specific variant context
    // pub fn calc_fst(&mut self, sample_indices: &[usize]) {
    //     let N_alleles = context.get_n_alleles();
    //
    //     let N_pops = sample_indices.len(); // treat each sample as a population
    //
    //     let ploidy = self.genotypes.get_max_ploidy(2) as usize; // check ploidy
    //     // Need to account for ploidy of genotypes here, not just diploid
    //     // let mut zygosity = vec![vec![]]
    //     let mut N_hom = Vec::new();
    //     let mut N_het = Vec::new();
    //     let mut n = vec![0.0; N_pops];
    //     let mut p = vec![vec![0.0; N_alleles]; N_pops];
    //
    //     let mut nbar = 0.0;
    //     let mut pbar = vec![0.0; N_alleles];
    //     let mut hbar = vec![0.0; N_alleles];
    //     let mut ssqr = vec![0.0; N_alleles];
    //     let mut sum_nsqr = 0.0;
    //     let mut n_sum = 0.0;
    //
    //     for i in 0..N_pops {
    //         self.get_multiple_genotype_counts(sample_indices, &mut N_hom, &mut N_het);
    //
    //         for j in 0..N_alleles {
    //             n[i] += N_hom[j] + 0.5 * N_het[j] as f64;
    //             p[i][j] = N_het[j] + 2.0 * N_hom[j] as f64;
    //
    //             nbar += n[i];
    //             pbar[j] += p[i][j];
    //             hbar[j] += N_het[j];
    //         }
    //
    //         for j in 0..N_alleles {
    //             p[i][j] /
    //         }
    //     }
    // }
    //
    // pub fn get_multiple_genotype_counts(
    //     &mut self,
    //     sample_indices: &[usize],
    //     out_N_hom: &mut Vec<usize>,
    //     out_N_het: &mut Vec<usize>
    // ) {
    //     out_N_het.fill(0);
    //     out_N_het.resize(self.get_n_alleles(), 0);
    //
    //     out_N_hom.fill(0);
    //     out_N_hom.resize(self.get_n_alleles(), 0);
    //
    //     for sample_index in sample_indices {
    //         let genotype_type: Option<&GenotypeType> =
    //             self.get_genotypes_mut().genotypes_mut()[sample_index].get_type();
    //
    //         // actual index of first allele in genotype i.e. most abundant allele
    //         let allele_index = match self.get_alternate_alleles_with_index().iter().filter_map(|a| {
    //             if a.1 == &self.get_genotypes().genotypes()[0].alleles[0] {
    //                 a.0
    //             }
    //         }).next() {
    //             Some(i) => i,
    //             None => continue
    //         };
    //
    //         match genotype_type {
    //             None => continue,
    //             Some(gt_type) => {
    //                 match gt_type {
    //                     GenotypeType::HomRef
    //                     | GenotypeType::HomVar => {
    //
    //                         out_N_hom[allele_index] += 1;
    //                     },
    //                     GenotypeType::Het => {
    //                         out_N_het[allele_index] += 1;
    //                     },
    //                     _ => continue,
    //                 }
    //             }
    //         }
    //     }
    // }
}
