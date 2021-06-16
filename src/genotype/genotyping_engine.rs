use model::allele_frequency_calculator::AlleleFrequencyCalculator;
use genotype::genotype_likelihoods::GenotypeLikelihoods;
use genotype::genotype_prior_calculator::GenotypePriorCalculator;
use model::variant_context::VariantContext;
use std::collections::BinaryHeap;
use model::allele_subsetting_utils::AlleleSubsettingUtils;
use utils::simple_interval::SimpleInterval;
use utils::vcf_constants::VCFConstants;
use genotype::genotype_builder::{GenotypeAssignmentMethod, GenotypesContext, Genotype};
use utils::quality_utils::QualityUtils;
use genotype::genotype_likelihood_calculators::GenotypeLikelihoodCalculators;
use utils::math_utils::MathUtils;
use model::allele_frequency_calculator_result::AFCalculationResult;
use model::variants::Allele;
use std::collections::HashMap;
use utils::assembly_based_caller_utils::AssemblyBasedCallerUtils;
use ordered_float::OrderedFloat;
use rayon::prelude::*;

pub struct GenotypingEngine {
    pub allele_frequency_calculator: AlleleFrequencyCalculator,
    number_of_genomes: usize,
    samples: Vec<String>,
    // the top of the queue is the upstream deletion that ends first
    // note that we can get away with ordering just by the end and not the contig as long as we preserve the invariant
    // that everything in this queue belongs to the same contig
    upstream_deletions_loc: BinaryHeap<SimpleInterval>,
    do_allele_specific_calcs: bool,
    genotype_assignment_method: GenotypeAssignmentMethod,
    use_posterior_probabilities_to_calculate_qual: bool,
    annotate_number_of_alleles_discovered: bool,
}

impl GenotypingEngine {
    pub const TOO_LONG_PL: usize = 100000;

    pub fn make(
        args: &clap::ArgMatches,
        samples: Vec<String>,
        do_allele_specific_calcs: bool,
        sample_ploidy: usize,
    ) -> GenotypingEngine {
        GenotypingEngine {
            allele_frequency_calculator: AlleleFrequencyCalculator::make_calculator(args),
            samples,
            do_allele_specific_calcs,
            number_of_genomes: samples.len() * sample_ploidy,
            upstream_deletions_loc: BinaryHeap::new(),
            genotype_assignment_method: GenotypeAssignmentMethod::from_args(args),
            use_posterior_probabilities_to_calculate_qual: args.is_present("use-posteriors-to-calculate-qual"),
            annotate_number_of_alleles_discovered: args.is_present("annotate-with-num-discovered-alleles")
        }
    }

    /**
     * Main entry function to calculate genotypes of a given VC with corresponding GL's that is shared across genotypers (namely UG and HC).
     *
     * Completes a variant context with genotype calls and associated annotations given the genotype likelihoods and
     * the model that need to be applied.
     *
     * @param self                               Input variant context to complete.
     * @return                                   VC with assigned genotypes
     */
    pub fn calculate_genotypes(
        &mut self,
        mut vc: VariantContext,
        ploidy: usize,
        gpc: &GenotypePriorCalculator,
        given_alleles: Vec<VariantContext>,
        stand_min_conf: f64

    ) -> Option<VariantContext> {
        if vc.has_too_many_alternative_alleles() || vc.get_n_samples() == 0 {
            return None
        }

        let mut reduced_vc: VariantContext;
        if VariantContext::MAX_ALTERNATE_ALLELES < (vc.alleles.len() - 1) {
            let alleles_to_keep = AlleleSubsettingUtils::calculate_most_likely_alleles(
                &mut vc,
                ploidy,
                VariantContext::MAX_ALTERNATE_ALLELES
            );

            let reduced_genotypes = if alleles_to_keep.len() == 1 {
                VariantContext::subset_to_ref_only(&mut vc, ploidy)
            } else {
                AlleleSubsettingUtils::subset_alleles(
                    &vc.get_genotypes(),
                    ploidy,
                    vc.get_alleles(),
                    &alleles_to_keep,
                    gpc,
                    &GenotypeAssignmentMethod::SetToNoCall,
                    vc.get_dp(),
                )
            };
            reduced_vc = vc.clone();
            reduced_vc.alleles = alleles_to_keep;
            reduced_vc.genotypes = reduced_genotypes;

        }

        //Calculate the expected total length of the PL arrays for this VC to warn the user in the case that they will be exceptionally large
        let max_pl_length = GenotypeLikelihoods::calc_num_likelihoods(
            reduced_vc.get_n_alleles(),
            ploidy
        );

        if max_pl_length >= GenotypingEngine::TOO_LONG_PL as i64 {
            warn!("Length of PL arrays for this Variant Context \
            (position: {}, allles: {}, ploidy: {}) is likely to reach {} so processing may take a long time",
                  vc.loc.get_start(), vc.get_n_alleles(), vc.genotypes.get_max_ploidy(ploidy), max_pl_length)
        }

        let af_result = self.allele_frequency_calculator.calculate(reduced_vc, ploidy);
        let output_alternative_alleles = GenotypingEngine::calculate_output_allele_subset(
            &af_result,
            &vc,
            &given_alleles,
            stand_min_conf
        );

        // note the math.abs is necessary because -10 * 0.0 => -0.0 which isn't nice
        let log10_confidence = if !output_alternative_alleles.site_is_monomorphic {
            af_result.log10_prob_only_ref_allele_exists() + 0.0
        } else {
            af_result.log10_prob_variant_present() + 0.0
        };

        // Add 0.0 removes -0.0 occurrences.
        let phred_scaled_confidence = (-10.0 * log10_confidence) + 0.0;

        // return a None if we don't pass the confidence cutoff or the most likely allele frequency is zero
        // skip this if we are already looking at a vc with NON_REF as the first alt allele i.e. if we are in GenotypeGVCFs
        if !GenotypingEngine::passes_emit_threshold(
            phred_scaled_confidence,
            stand_min_conf,
            output_alternative_alleles.site_is_monomorphic,
        ) && GenotypingEngine::no_alleles_or_first_allele_is_not_non_ref(&output_alternative_alleles.alleles)
            && given_alleles.is_empty() {
            return None
        }

        // start constructing the resulting VC
        let output_alleles = output_alternative_alleles.output_alleles(vc.get_reference());

        self.record_deletions(&vc, &output_alleles);

        let mut builder = VariantContext::build(vc.loc.get_contig(), vc.loc.get_start(), vc.loc.get_end(), output_alleles);

        builder.log10_p_error(log10_confidence);
        if !GenotypingEngine::passes_call_threshold(phred_scaled_confidence, stand_min_conf) {
            builder.filter(VCFConstants::LOW_QUAL_FILTER_NAME)
        }

        // create the genotypes
        //TODO: omit subsetting if output alleles is not a proper subset of vc.getAlleles
        let genotypes = if output_alleles.len() == 1 {
            VariantContext::subset_to_ref_only(&mut vc, ploidy)
        } else {
            AlleleSubsettingUtils::subset_alleles(
                &vc.get_genotypes(),
                ploidy,
                vc.get_alleles(),
                &output_alleles,
                gpc,
                &self.genotype_assignment_method,
                vc.get_dp(),
            )
        };

        if self.use_posterior_probabilities_to_calculate_qual && GenotypingEngine::has_posteriors(&genotypes) {
            let log10_no_variant_posterior = GenotypingEngine::phred_no_variant_posterior_probability(&output_alleles, &mut genotypes) * (-0.1);

            let qual_update = if !output_alternative_alleles.site_is_monomorphic { // TODO: add || self.annotate_all_sites_with_pls
                log10_no_variant_posterior + 0.0
            } else {
                MathUtils::log10_one_minus_pow10(log10_no_variant_posterior) + 0.0
            };

            if !qual_update.is_nan() {
                builder.log10_p_error(qual_update)
            }
        }

        // calculating strand bias involves overwriting data structures, so we do it last
        let attributes = self.compose_call_attributes(
            &vc,
            &output_alternative_alleles.alternative_allele_mle_counts(),
            &af_result,
            &output_alternative_alleles.output_alleles(vc.get_reference()),
            &genotypes
        );

        builder.attributes(attributes);


        return Some(builder)
    }

    fn phred_no_variant_posterior_probability(alleles: &Vec<Allele>, gc: &mut GenotypesContext) -> f64 {
        gc.genotypes().par_iter()
            .map(|gt| GenotypingEngine::extract_p_no_alt(alleles, &mut gt))
            .filter(|d| !d.is_nan())
            .fold(|| std::f64::NAN, |a, b| {
                if d.is_nan() {
                    b
                } else if b.is_nan() {
                    a
                } else {
                    a + b
                }
            }).sum::<f64>()
    }

    fn extract_p_no_alt(allele: &Vec<Allele>, gt: &mut Genotype) -> f64 {
        let gp_array = gt.get_attribute(&VCFConstants::GENOTYPE_POSTERIORS_KEY);

        GenotypingEngine::extract_p_no_alt_with_posteriors(alleles: &Vec<Allele>, gt: &mut Genotype, gp_array)
    }

    fn extract_p_no_alt_with_posteriors(alleles: &Vec<Allele>, gt: &mut Genotype, posteriors: &mut [f64]) -> f64 {
        if !alleles.par_iter().any(|allele| allele.is_del()) {
            let reducer: f64 = std::cmp::max(OrderedFloat(0.0), OrderedFloat(QualityUtils::phred_sum(posteriors))).into();
            return posteriors[0] - reducer
        } else {
            // here we need to get indices of genotypes composed of REF and * alleles
            let ploidy = gt.ploidy;
            let gl_calc = GenotypeLikelihoodCalculators::get_instance(ploidy, alleles.len());
            let span_del_index = alleles.par_iter().position_first(|allele| allele.is_del()).unwrap();
            // allele counts are in the GenotypeLikelihoodCalculator format of {ref index, ref count, span del index, span del count}
            let mut non_variant_log10_posteriors = (0..ploidy).into_iter().map(|n| {
                gl_calc.allele_counts_to_index(&vec![0, ploidy - n, span_del_index, n]);
                posteriors[n]
            }).collect::<Vec<f64>>();

            // when the only alt allele is the spanning deletion the probability that the site is non-variant
            // may be so close to 1 that finite precision error in log10SumLog10 (called by phredSum) yields a positive value,
            // which is bogus.  Thus we cap it at 0. See AlleleFrequencyCalculator.
            return (std::cmp::max(OrderedFloat(0.0), OrderedFloat(QualityUtils::phred_sum(&mut non_variant_log10_posteriors)))
                - std::cmp::max(OrderedFloat(0.0), OrderedFloat(QualityUtils::phred_sum(posteriors)))).into()
        }
    }

    /**
     *  Record emitted deletions in order to remove downstream spanning deletion alleles that are not covered by any emitted deletion.
     *  In addition to recording new deletions, this method culls previously-recorded deletions that end before the current variant
     *  context.  This assumes that variants are traversed in order.
     *
     * @param vc                VariantContext, potentially multiallelic and potentially containing one or more deletion alleles
     * @param emittedAlleles    The subset of the variant's alt alleles that are actually emitted
     */
    fn record_deletions(&mut self, vc: &VariantContext, emitted_alleles: &Vec<Allele>) {
        while !self.upstream_deletions_loc.is_empty()
            && (!self.upstream_deletions_loc.peek().unwrap_or(&SimpleInterval::new(0, 0, 0)).contigs_match(&vc.loc)
            || self.upstream_deletions_loc.peek().unwrap_or(&SimpleInterval::new(0, 0, 0)).get_end() < vc.loc.get_start()) {
            self.upstream_deletions_loc.pop();
        }

        for allele in emitted_alleles.iter() {
            let deletion_size = vc.get_reference().length() - allele.length();

            if deletion_size > 0 {
                let genome_loc = SimpleInterval::new(vc.loc.get_contig(), vc.loc.get_start(), vc.loc.get_start() + deletion_size);
                self.upstream_deletions_loc.push(genome_loc);
            }
        }
    }

    fn no_alleles_or_first_allele_is_not_non_ref(alt_alleles: &Vec<Allele>) -> bool {
        alt_alleles.is_empty() || alt_alleles[0] != Allele::NON_REF_ALLELE
    }

    pub fn passes_emit_threshold(conf: f64, min_conf: f64, best_guess_is_ref: bool) -> bool {
        !best_guess_is_ref && GenotypingEngine::passes_call_threshold(conf, min_conf)
    }

    pub fn passes_call_threshold(conf: f64, min_conf: f64) -> bool {
        conf >= min_conf
    }

    /**
     * Provided the exact mode computations it returns the appropriate subset of alleles that progress to genotyping.
     * @param afCalculationResult the allele fraction calculation result.
     * @param vc the variant context
     * @return information about the alternative allele subsetting {@code null}.
     */
    fn calculate_output_allele_subset(
        af_calculation_result: &AFCalculationResult,
        vc: &VariantContext, given_alleles: &Vec<VariantContext>,
        stand_min_conf: f64
    ) -> OutputAlleleSubset {
        let mut output_alleles = Vec::new();
        let mut mle_counts = Vec::new();

        let mut site_is_monomorphic = true;
        let alleles = af_calculation_result.get_alleles_used_in_genotyping();
        let alternative_allele_count = alleles.len() - 1;
        let mut reference_size = 0;

        let forced_alleles = AssemblyBasedCallerUtils::get_alleles_consistent_with_given_alleles(given_alleles, vc);

        for allele in alleles.iter() {
            if allele.is_reference() {
                reference_size = allele.length();
            } else {
                // we want to keep the NON_REF symbolic allele but only in the absence of a non-symbolic allele, e.g.
                // if we combined a ref / NON_REF gVCF with a ref / alt gVCF
                let is_non_ref_which_is_lone_alt_allele = alternative_allele_count == 1 && allele.eq(&Allele::NON_REF_ALLELE);
                let is_plausible = af_calculation_result.passes_threshold(allele, stand_min_conf);

                //it's possible that the upstream deletion that spanned this site was not emitted, mooting the symbolic spanning deletion allele
                let is_spurious_spanning_deletion = allele.is_del();

                let to_output = is_plausible || is_non_ref_which_is_lone_alt_allele || forced_alleles.contains(allele);

                site_is_monomorphic = site_is_monomorphic & !(is_plausible);

                if to_output {
                    output_alleles.push(allele.clone());
                    mle_counts.push(af_calculation_result.get_allele_count_at_mle(allele));
                }
            }
        }

        return OutputAlleleSubset::new(output_alleles, mle_counts, site_is_monomorphic)
    }

    fn has_posteriors(gc: &GenotypesContext) -> bool {
        gc.genotypes().par_iter().any(|genotype| genotype.has_attribute(&VCFConstants::GENOTYPE_POSTERIORS_KEY))
    }

    fn compose_call_attributes(
        &self,
        vc: &VariantContext,
        allele_counts_of_mle: &Vec<i64>,
        af_result: &AFCalculationResult,
        all_alleles_to_use: &Vec<Allele>,
        genotypes: &GenotypesContext
    ) -> HashMap<String, Vec<f64>> {
        let mut attributes = HashMap::new();

        // add the MLE AC and AF annotations
        if !allele_counts_of_mle.is_empty() {
            attributes.insert(VCFConstants::MLE_ALLELE_COUNT_KEY, allele_counts_of_mle.par_iter().map(|count| *count as f64).collect::<Vec<f64>>());
            let mle_frequencies = GenotypingEngine::calculate_mle_allele_frequencies(
                &allele_counts_of_mle,
                genotypes
            );
            attributes.insert(VCFConstants::MLE_ALLELE_FREQUENCY_KEY, mle_frequencies);
        }

        if self.do_allele_specific_calcs {
            let mut per_allele_quals = Vec::new();
            //Per-allele quals are not calculated for biallelic sites
            if af_result.get_alleles_used_in_genotyping().len() > 2 {
                for a in all_alleles_to_use.iter() {
                    if !a.is_reference() {
                        //*-10 to convert from log10-scale to Phred-scale, as QUALs are typically represented
                        per_allele_quals.push(af_result.get_log10_posterior_of_allele_absent(a) * -10.0);
                    }
                }
            } else {
                //*-10 to convert from log10-scale to Phred-scale, as QUALs are typically represented
                per_allele_quals.push(af_result.log10_prob_only_ref_allele_exists() * -10.0);
            }
            attributes.insert(VCFConstants::AS_QUAL_KEY, per_allele_quals);
        }

        if self.annotate_number_of_alleles_discovered {
            attributes.insert(VCFConstants::NUMBER_OF_DISCOVERED_ALLELES_KEY, vec![vc.get_alternate_alleles().len() as f64]);
        }

        return attributes
    }

    fn calculate_mle_allele_frequencies(allele_counts_of_mle: &[i64], genotypes: &GenotypesContext) -> Vec<f64> {
        let an = genotypes.genotypes().iter().flat_map(|g| {
            g.alleles.iter()
        }).filter(|a| a.is_called()).count();

        return allele_counts_of_mle.par_iter().map(|ac| {
            std::cmp::min(OrderedFloat(0.0), OrderedFloat((*ac as f64) / (an as f64))).into()
        }).collect::<Vec<f64>>()
    }
}

struct OutputAlleleSubset {
    alleles: Vec<Allele>,
    site_is_monomorphic: bool,
    mle_counts: Vec<i64>
}

impl OutputAlleleSubset {
    pub fn new(alleles: Vec<Allele>, mle_counts: Vec<i64>, site_is_monomorphic: bool) -> OutputAlleleSubset {
        OutputAlleleSubset {
            alleles,
            mle_counts,
            site_is_monomorphic
        }
    }

    pub fn output_alleles(&self, reference_allele: &Allele) -> Vec<Allele> {
        let mut result = self.alleles.clone();
        result.insert(0, reference_allele.clone());
        return result
    }

    pub fn alternative_allele_mle_counts(&self) -> Vec<i64> {
        self.mle_counts.clone()
    }
}