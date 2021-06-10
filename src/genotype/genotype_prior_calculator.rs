use enum_ordinalize;
use utils::math_utils::MathUtils;

#[derive(Debug, PartialEq, Eq, Ordinalize)]
enum AlleleType {
    REF,
    SNP,
    INDEL,
    OTHER,
}

/**
 * Class to compose genotype prior probability calculators.
 *
 * <p>
 *     Contains a collection of static method to create calculators based on different
 *     assumptions and source of knowledge a priori (e.g. {@link #assumingHW(double, double) assumingHW}
 *     or {@link #givenDragstrParams(DragstrParams, int, int, double, double) givenDragstrParams}).
 * </p>
 *
 * <p>
 *     Such priors are obtained by invoking {@link #getLog10Priors(GenotypeLikelihoodCalculator, List).
 *     This method takes on the list of alleles for that variant, an a reference to the genotype likelihood calculator witch determines the ploidy.
 * </p>
 * assumptions
 */
pub struct GenotypePriorCalculator {
    het_values: Vec<f64>,
    hom_values: Vec<f64>,
    diff_values: Vec<f64>
}

impl GenotypePriorCalculator {
    const NUMBER_OF_ALLELE_TYPES: usize = 4;

    // A snp can go to 3 different bases (standard-nucs - 1), so we normalize SNP lks accordingly. Here is the
    // log10 constant used for that:
    const LOG10_SNP_NORMALIZATION_CONSTANT: f64 = (3. as f64).log10();

    fn genotype_prior_calculator(
        snp_het: f64,
        snp_hom: f64,
        indel_het: f64,
        indel_hom: f64,
        other_het: f64,
        other_hom: f64
    ) -> GenotypePriorCalculator {
        let mut het_values = vec![0.; GenotypePriorCalculator::NUMBER_OF_ALLELE_TYPES];
        let mut hom_values = vec![0.; GenotypePriorCalculator::NUMBER_OF_ALLELE_TYPES];

        // REF
        // by convention ref log10 likelihoods are set to 0.
        // so they are already set.

        // SNPs: normalized for all possible mutations (number of nucs (4) - 1)
        het_values[AlleleType::SNP.ordinal()] = snp_het - GenotypePriorCalculator::LOG10_SNP_NORMALIZATION_CONSTANT;
        hom_values[AlleleType::SNP.ordinal()] = snp_hom - GenotypePriorCalculator::LOG10_SNP_NORMALIZATION_CONSTANT;
        // INDELs:
        het_values[AlleleType::INDEL.ordinal()] = indel_het;
        hom_values[AlleleType::INDEL.ordinal()] = indel_hom;
        // Others:
        het_values[AlleleType::OTHER.ordinal()] = other_het;
        hom_values[AlleleType::OTHER.ordinal()] = other_hom;

        let diff_values = MathUtils::ebe_subtract(&hom_values, &het_values);

        GenotypePriorCalculator {
            het_values,
            hom_values,
            diff_values
        }
    }

    /**
     * Calculate priors based on fix heterozygosities (per event type) and het to hom-var prior ratio.
     *
     * @param log10SnpHet snp heterozygosity in log10 scale.
     * @param log10IndelHet indel heterozygosity in log10 scale.
     * @param log10OtherHet heterozygosity for other type of variants in log10 scale.
     * @param hetHomRatio ratio between the het-var and hom-var genotype priors for the same allele in linear scale.
     * @return never {@code null}.
     */
    pub fn given_het_to_hom_ratio(
        log10_snp_het: f64,
        log10_indel_het: f64,
        log10_other_het: f64,
        het_hom_ratio: f64
    ) -> GenotypePriorCalculator {
        let log10_ratio = het_hom_ratio.log10();

        GenotypePriorCalculator::genotype_prior_calculator(
            log10_snp_het, log10_snp_het - log10_ratio,
            log10_indel_het, log10_indel_het - log10_ratio,
            log10_other_het, log10_other_het - log10_ratio
        )
    }

    /**
     * Composes a calculator based on Hardy-Weinberg equilibrium so that only the het-priors
     * are need to calculate the rest.
     * @param snpHet the prior for an SNP alternative allele in log10 scale.
     * @param indelHet the prior for an INDEL alternative allele in log10 scale.
     * @return never {@code null}.
     */
    pub fn assuming_hw(
        snp_het: f64,
        indel_het: f64,
        other_het: Option<f64>
    ) -> GenotypePriorCalculator {
        match other_het {
            Some(other) => {
                GenotypePriorCalculator::genotype_prior_calculator(
                    snp_het, snp_het * 2.,
                    indel_het, indel_het * 2.,
                    other, other * 2.,
                )
            },
            None => {
                GenotypePriorCalculator::genotype_prior_calculator(
                    snp_het, snp_het * 2.,
                    indel_het, indel_het * 2.,
                    std::cmp::max(snp_het, indel_het),
                )
            }
        }

    }

    pub fn make(args: &clap::ArgMatches) -> GenotypePriorCalculator {
        let snp_het = args.value_of("snp-heterozygosity").unwrap().parse::<f64>().unwrap();
        let ind_het = args.value_of("indel-heterozygosity").unwrap().parse::<f64>().unwrap();

        GenotypePriorCalculator::assuming_hw(snp_het, ind_het, None)
    }

}

