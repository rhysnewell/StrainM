use model::allele_list::AlleleList;
use model::variants::Allele;
use genotype::genotype_builder::Genotype;
use model::variant_context::VariantContext;
use clap::ArgMatches;
use genotype::genotype_likelihood_calculator::GenotypeLikelihoodCalculator;
use utils::math_utils::MathUtils;
use utils::dirichlet::Dirichlet;
use ordered_float::OrderedFloat;
use genotype::genotype_likelihood_calculators::GenotypeLikelihoodCalculators;
use model::allele_frequency_calculator_result::AFCalculationResult;

pub struct AlleleFrequencyCalculator {
    pub ref_pseudo_count: f64,
    pub snp_pseudo_count: f64,
    pub indel_pseudo_count: f64,
    pub default_ploidy: usize,
}

impl AlleleFrequencyCalculator {
    const GL_CALCS: GenotypeLikeliHoodCalculators = GenotypeLikelihoodCalculators::build_empty();
    const THRESHOLD_FOR_ALLELE_COUNT_CONVERGENCE: f64 = 0.1;
    const HOM_REF_GENOTYPE_INDEX: usize = 0;

    pub fn new(
        ref_pseudo_count: f64,
        snp_pseudo_count: f64,
        indel_pseudo_count: f64,
        default_ploidy: usize
    ) -> AlleleFrequencyCalculator {
        AlleleFrequencyCalculator {
            ref_pseudo_count,
            snp_pseudo_count,
            indel_pseudo_count,
            default_ploidy
        }
    }

    pub fn make_calculator(args: &ArgMatches) -> AlleleFrequencyCalculator {
        let snp_het = args.value_of("snp-heterozygosity").unwrap().parse::<f64>().unwrap();
        let ind_het = args.value_of("indel-heterozygosity").unwrap().parse::<f64>().unwrap();
        let het_std = args.value_of("heterozygosity-stdev").unwrap().parse::<f64>().unwrap();
        let ploidy: usize = m.value_of("ploidy").unwrap().parse().unwrap();

        let ref_pseudo_count = snp_het / (het_std.powf(2.));
        let snp_pseudo_count = snp_het * ref_pseudo_count;
        let indel_pseudo_count = ind_het * ref_pseudo_count;

        AlleleFrequencyCalculator::new(
            ref_pseudo_count,
            snp_pseudo_count,
            indel_pseudo_count,
            ploidy
        )
    }

    fn log10_normalized_genotype_posteriors<T: Float + Copy>(
        &mut self,
        g: &mut Genotype,
        gl_calc: &mut GenotypeLikelihoodCalculator,
        log10_allele_frequencies: &mut [T],
    ) -> Vec<T> {
        let mut log10_likelihoods = g.get_likelihoods();
        let log10_posteriors =
            (0..gl_calc.genotype_count as usize).iter().map(|genotype_index| {
                let mut gac = gl_calc.genotype_allele_counts_at(genotype_index);
                let result = gac.log10_combination_count()
                    + log10_likelihoods[genotype_index]
                    + gac.sum_over_allele_indices_and_counts(
                    |index: usize, count: usize| {
                        (count as T) * &log10_allele_frequencies[index]
                    }
                );
                result
            }).collect::<Vec<T>>();

        return MathUtils::normalize_log10(log10_posteriors, true);
    }

    /**
     * Calculate the posterior probability that a single biallelic genotype is non-ref
     *
     * The nth genotype (n runs from 0 to the sample ploidy, inclusive) contains n copies of the alt allele
     * @param log10GenotypeLikelihoods
     */
    pub fn calculate_single_sample_biallelic_non_ref_posterior(
        &self,
        log10_genotype_likelihoods: &Vec<OrderedFloat<f64>>,
        return_zero_if_ref_is_max: bool,
    ) -> f64 {
        if return_zero_if_ref_is_max
            && log10_genotype_likelihoods.iter()
            .position(|&item| item == *(log10_genotype_likelihoods.iter().max().unwrap())).unwrap() == 0
            && (log10_genotype_likelihoods[0] != OrderedFloat(0.5) && log10_genotype_likelihoods.len() == 2){
            return 0.
        }

        let ploidy = log10_genotype_likelihoods.len() - 1;

        let log10_unnormalized_posteriors = (0..ploidy + 1)
            .iter()
            .map(|n| {
                log10_genotype_likelihoods[n]
                    + MathUtils::log10_binomial_coeffecient(ploidy as f64, n as f64)
                    + MathUtils::log_to_log10(
                    (n as f64 + self.snp_pseudo_count).log_gamma()
                        + ((ploidy - n) as f64 + self.ref_pseudo_count).log_gamma()
                )
            });

        return if return_zero_if_ref_is_max
            && MathUtils::max_element_index(log10_unnormalized_posteriors, 0, log10_unnormalized_posteriors.len()) == 0 {
            0.0
        } else {
            1 - MathUtils::normalize_log10(log10_unnormalized_posteriors, false)[0]
        }
    }

    /**
     * Compute the probability of the alleles segregating given the genotype likelihoods of the samples in vc
     *
     * @param vc the VariantContext holding the alleles and sample information.  The VariantContext
     *           must have at least 1 alternative allele
     * @return result (for programming convenience)
     */
    pub fn calculate(&mut self, vc: VariantContext, default_ploidy: usize) -> AFCalculationResult {
        let num_alleles = vc.get_n_alleles();
        let alleles = vc.get_alleles();
        if num_alleles <= 1 {
            panic!("Variant context has only a dingle reference allele, but get_log10_p_non_ref requires at least one at all {:?}", vc);
        }
        let prior_pseudo_counts = alleles.par_iter().map(|a| {
            if a.is_reference() {
                self.ref_pseudo_count
            } else if a.length() == vc.get_reference().length() {
                self.snp_pseudo_count
            } else {
                self.indel_pseudo_count
            }
        }).collect_vec::<Vec<f64>>();

        let mut allele_counts = vec![0.0; num_alleles];
        let flat_log10_allele_frequency = -((num_alleles as f64).log10());
        let mut log10_allele_frequencies = vec![flat_log10_allele_frequency; num_alleles];

        let mut allele_counts_maximum_difference = std::f64::INFINITY;
        while allele_counts_maximum_difference > AlleleFrequencyCalculator::THRESHOLD_FOR_ALLELE_COUNT_CONVERGENCE {
            let new_allele_counts = self.effective_allele_counts(&vc, &mut log10_allele_frequencies);
            allele_counts_maximum_difference =
                MathUtils::ebe_subtract(&allele_counts, &new_allele_counts).par_iter_mut().for_each(|x| {
                    *x = x.abs()
                }).max();
            allele_counts = new_allele_counts;

            let posterior_pseudo_counts = MathUtils::ebe_add(&prior_pseudo_counts, &allele_counts);

            // first iteration uses flat prior in order to avoid local minimum where the prior + no pseudocounts gives such a low
            // effective allele frequency that it overwhelms the genotype likelihood of a real variant
            // basically, we want a chance to get non-zero pseudocounts before using a prior that's biased against a variant
            log10_allele_frequencies = Dirichlet::new(&posterior_pseudo_counts).log10_mean_weights();
        }

        let log10_p_of_zero_counts_by_allele = vec![0.0; num_alleles];
        let mut log10_p_no_variant = 0.0;

        let spanning_deletion_present = alleles.par_iter().any(|allele| allele.is_del());

        let non_variant_indices_by_ploidy = BTreeMap::new();

        // re-usable buffers of the log10 genotype posteriors of genotypes missing each allele
        let mut log10_absent_posteriors = vec![Vec::new(); num_alleles];

        for genotype in vc.get_genotypes().iter_mut() {
            if !g.has_likelihoods() {
                continue
            }

            let ploidy = if g.get_ploidy == 0 {
                default_ploidy
            } else {
                g.get_ploidy()
            };

            let mut gl_calc = GenotypeLikelihoodCalculators::get_instance(ploidy, num_alleles);

            let log10_genotype_posteriors = self.log10_normalized_genotype_posteriors(
                &mut g,
                &mut gl_calc,
                &mut log10_allele_frequencies,
            );

            if !spanning_deletion_present {
                log10_p_no_variant += log10_genotype_posteriors[AlleleFrequencyCalculator::HOM_REF_GENOTYPE_INDEX];
            } else {
                let non_variant_indices = non_variant_indices_by_ploidy.entry(ploidy)
                    .or_insert(AlleleFrequencyCalculator::genotype_indices_with_only_ref_and_span_del(
                        ploidy,
                        &alleles
                    ));

                let non_variant_log10_posteriors = non_variant_indices.par_iter().map(|n| {
                    log10_genotype_posteriors[n]
                }).collect_vec();
                // when the only alt allele is the spanning deletion the probability that the site is non-variant
                // may be so close to 1 that finite precision error in log10SumLog10 yields a positive value,
                // which is bogus.  Thus we cap it at 0.
                log10_p_no_variant += std::cmp::min(
                    0,
                    MathUtils::log10_sum_log10(&non_variant_log10_posteriors, 0, non_variant_log10_posteriors.len()));
            }

            // if the VC is biallelic the allele-specific qual equals the variant qual
            if num_alleles == 2 && !spanning_deletion_present {
                continue
            }

            // for each allele, we collect the log10 probabilities of genotypes in which the allele is absent, then add (in log space)
            // to get the log10 probability that the allele is absent in this sample
            log10_absent_posteriors.par_iter_mut().for_each(|arr| {
                *arr.clear()
            });
            for genotype in (0..gl_calc.genotype_count) {
                let log10_genotype_posterior = log10_genotype_posteriors[genotype];
                gl_calc.genotype_allele_counts_at(genotype)
                    .for_each_absent_allele_index(|a| {
                        log10_absent_posteriors[a].push(log10_genotype_posterior)
                    }, num_alleles);
            }

            let log10_p_no_allele = log10_absent_posteriors.par_iter().map(|buffer| {
                let mut result = MathUtils::log10_sum_log10(&buffer, 0, buffer.len());
                result = std::cmp::min(0, result);
                result
            }).collect_vec();

            // multiply the cumulative probabilities of alleles being absent, which is addition of logs
            MathUtils::ebe_add_in_place(&mut log10_p_of_zero_counts_by_allele, &log10_p_no_allele);
        }

        // for biallelic the allele-specific qual equals the variant qual, and we short-circuited the calculation above
        if num_alleles == 2 && !spanning_deletion_present {
            log10_p_of_zero_counts_by_allele[1] = log10_p_no_variant
        }

        let int_allele_counts = allele_counts.par_iter().map(|n| n as i64).collect_vec();
        let int_alt_allele_counts = int_allele_counts[1..].clone();
        let log10_p_ref_by_allele = (1..num_alleles).into_par_iter().map(|a| {
            (alleles[a], log10_p_of_zero_counts_by_allele[a])
        }).collect::<HashMap<Allele, f64>>();

        return AFCalculationResult::new(int_alt_allele_counts, allele, log10_p_no_variant, log10_p_ref_by_allele)

    }

    fn genotype_indices_with_only_ref_and_span_del(ploidy: usize, alleles: &Vec<Allele>) -> Vec<usize> {
        let gl_calc = GenotypeLikelihoodCalculators::get_instance(ploidy, alleles.len());

        let spanning_deletion_present = alleles.par_iter().any(|allele| allele.is_del());

        if !spanning_deletion_present {
            vec![AlleleFrequencyCalculator::HOM_REF_GENOTYPE_INDEX]
        } else {
            let span_del_index = alleles.par_iter().position(|allele| allele.is_del()).unwrap();
            let result = (0..ploidy + 1).into_iter().map(|n| {
                gl_calc.allele_counts_to_index(vec![0, ploidy - n, span_del_index, n])
            }).collect_vec();

            return result
        }
    }

    /**
    * effectiveAlleleCounts[allele a] = SUM_{genotypes g} (posterior_probability(g) * num_copies of a in g), which we denote as SUM [n_g p_g]
    * for numerical stability we will do this in log space:
    * count = SUM 10^(log (n_g p_g)) = SUM 10^(log n_g + log p_g)
    * thanks to the log-sum-exp trick this lets us work with log posteriors alone
    */
    fn effective_allele_counts<T: Float + Copy>(&mut self, vc: &VariantContext, log10_allele_frequencies: &mut [T]) -> Vec<T> {
        let num_alleles = vc.get_n_alleles();
        let mut log10_result = vec![std::f64::NEG_INFINITY; num_alleles];
        for g in vc.get_genotypes().iter_mut() {
            if !g.has_likelihoods() {
                continue
            }
            let mut gl_calc = GenotypeLikelihoodCalculators::get_instance(g.get_ploidy(), num_alleles);

            let log10_genotype_posteriors = self.log10_normalized_genotype_posteriors(&mut g, &mut gl_calc, &mut log10_allele_frequencies);

            (0..gl_calc.genotype_count).into_iter().for_each(|genotype_index|{
                gl_calc.genotype_allele_counts_at(genotype_index).for_each_allele_index_and_count(
                    |allele_index: usize, count: usize| {
                    log10_result[allele_index] =
                        MathUtils::log10_sum_log10_two_values(
                            log10_result[allele_index],
                            log10_genotype_posteriors[genotype_index] + (count as T).log10()
                        )
                })
            });
        }
        log10_result.par_iter_mut().for_each(|x| {
            *x = (10.0).powf(x)
        });
        return log10_result
    }
}