use ndarray::Array2;
use genotype::genotype_allele_counts::GenotypeAlleleCounts;
use genotype::genotype_likelihood_calculators::GenotypeLikelihoodCalculators;
use std::collections::BinaryHeap;

#[derive(Clone, Debug)]
pub struct GenotypeLikelihoodCalculator {
    /**
     * Number of genotypes given this calculator {@link #ploidy} and {@link #alleleCount}.
     */
    pub genotype_count: i32,
    /**
     * Ploidy for this calculator.
     */
    pub ploidy: usize,
    /**
     * Number of genotyping alleles for this calculator.
     */
    pub allele_count: usize,
    /**
     * Offset table for this calculator.
     *
     * <p>
     *     This is a shallow copy of {@link GenotypeLikelihoodCalculators#alleleFirstGenotypeOffsetByPloidy} when the calculator was created
     *     thus it follows the same format as that array. Please refer to its documentation.
     * </p>
     *
     * <p>You can assume that this offset table contain at least (probably more) the numbers corresponding to the allele count and ploidy for this calculator.
     * However since it might have more than that and so you must use {@link #alleleCount} and {@link #ploidy} when
     * iterating through this array rather that its length or the length of its components.</p>.
     */
    pub allele_first_genotype_offset_by_ploidy: Array2<i32>,
    /**
     * Genotype table for this calculator.
     *
     * <p>It is ensure that it contains all the genotypes for this calculator ploidy and allele count, maybe more. For
     * that reason you must use {@link #genotypeCount} when iterating through this array and not relay on its length.</p>
     */
    pub genotype_allele_counts: Vec<GenotypeAlleleCounts>,
    /**
     * Buffer used as a temporary container for likelihood components for genotypes stratified by reads.
     *
     * <p>
     *     It is indexed by genotype index and then by read index. The read capacity is increased as needed by calling
     *     {@link #ensureReadCapacity(int) ensureReadCapacity}.
     * </p>
     */
    pub read_likelihoods_by_genotype_index: Vec<Vec<f64>>,
    /**
     * Buffer field use as a temporal container for sorted allele counts when calculating the likelihood of a
     * read in a genotype.
     * <p>
     *      This array follows the same format as {@link GenotypeAlleleCounts#sortedAlleleCounts}. Each component in the
     *      genotype takes up two positions in the array where the first indicate the allele index and the second its frequency in the
     *      genotype. Only non-zero frequency alleles are represented, taking up the first positions of the array.
     * </p>
     *
     * <p>
     *     This array is sized so that it can accommodate the maximum possible number of distinct alleles in any
     *     genotype supported by the calculator, value stored in {@link #maximumDistinctAllelesInGenotype}.
     * </p>
     */
    pub genotype_alleles_and_counts: Vec<i32>,
    /**
     * Maximum number of components (or distinct alleles) for any genotype with this calculator ploidy and allele count.
     */
    pub maximum_distinct_alleles_in_genotype: usize,
    /**
     * Cache of the last genotype-allele-count requested using {@link #genotypeAlleleCountsAt(int)}, when it
     * goes beyond the maximum genotype-allele-count static capacity. Check on that method documentation for details.
     */
    pub last_overhead_counts: GenotypeAlleleCounts,
    /**
     * Indicates how many reads the calculator supports.
     *
     * <p>This figure is increased dynamically as per the
     * calculation request calling {@link #ensureReadCapacity(int) ensureReadCapacity}.<p/>
     */
    pub read_capacity: i32,
    /**
     * Buffer field use as a temporal container for component likelihoods when calculating the likelihood of a
     * read in a genotype. It is stratified by read and the allele component of the genotype likelihood... that is
     * the part of the likelihood sum that correspond to a particular allele in the genotype.
     *
     * <p>
     *     It is implemented in a 1-dimensional array since typically one of the dimensions is rather small. Its size
     *     is equal to {@link #readCapacity} times {@link #maximumDistinctAllelesInGenotype}.
     * </p>
     *
     * <p>
     *     More concretely [r][i] == log10Lk(read[r] | allele[i]) + log(freq[i]) where allele[i] is the ith allele
     *     in the genotype of interest and freq[i] is the number of times it occurs in that genotype.
     * </p>
     */
    read_genotype_likelihood_components: Vec<f64>,

    /**
    * Max-heap for integers used for this calculator internally.
    */
    allele_heap: BinaryHeap<usize>,
}

impl GenotypeLikelihoodCalculator {
    pub fn new(
        ploidy: usize,
        allele_count: usize,
        allele_first_genotype_offset_by_ploidy: Array2<i32>,
        genotype_table_by_ploidy: Vec<Vec<GenotypeAlleleCounts>>,
    ) -> GenotypeLikelihoodCalculator {
        let genotype_count = allele_first_genotype_offset_by_ploidy[[ploidy, allele_count]];
        let maximum_distinct_alleles_in_genotype = std::cmp::min(ploidy, allele_count);
        GenotypeLikelihoodCalculator {
            genotype_allele_counts: genotype_table_by_ploidy[ploidy].clone(),
            read_likelihoods_by_genotype_index: vec![Vec::new(); genotype_count as usize],
            genotype_alleles_and_counts: vec![0; maximum_distinct_alleles_in_genotype as usize * 2],
            maximum_distinct_alleles_in_genotype,
            last_overhead_counts: GenotypeAlleleCounts::build_empty(),
            read_capacity: -1,
            genotype_count,
            allele_count,
            ploidy,
            allele_first_genotype_offset_by_ploidy,
            read_genotype_likelihood_components: vec![],
            allele_heap: BinaryHeap::with_capacity(ploidy),
        }
    }

    /**
     * Makes sure that temporal arrays and matrices are prepared for a number of reads to process.
     * @param requestedCapacity number of read that need to be processed.
     */

    /**
     * Returns the genotype associated to a particular likelihood index.
     *
     * <p>If {@code index} is larger than {@link GenotypeLikelihoodCalculators#MAXIMUM_STRONG_REF_GENOTYPE_PER_PLOIDY},
     *  this method will reconstruct that genotype-allele-count iteratively from the largest strongly referenced count available.
     *  or the last requested index genotype.
     *  </p>
     *
     * <p> Therefore if you are iterating through all genotype-allele-counts you should do sequentially and incrementally, to
     * avoid a large efficiency drop </p>.
     *
     * @param index query likelihood-index.
     * @return never {@code null}.
     */
    pub fn genotype_allele_counts_at(&mut self, index: usize) -> GenotypeAlleleCounts {
        if !(index >= 0 && index < self.genotype_count as usize) {
            panic!("Invalid likelihood index {} >= {} (Genotype count for n-alleles = {} and {}",
                    index, self.genotype_count, self.allele_count, self.ploidy);
        } else {
            if index < GenotypeLikelihoodCalculators::MAXIMUM_STRONG_REF_GENOTYPE_PER_PLOIDY as usize {
                return self.genotype_allele_counts[index].clone()
            } else if self.last_overhead_counts.is_null()
                || self.last_overhead_counts.index() > index {
                let mut result = self.genotype_allele_counts[(GenotypeLikelihoodCalculators::MAXIMUM_STRONG_REF_GENOTYPE_PER_PLOIDY - 1) as usize].clone();

                result.increase(index as i32 - GenotypeLikelihoodCalculators::MAXIMUM_STRONG_REF_GENOTYPE_PER_PLOIDY + 1);
                self.last_overhead_counts = result.clone();
                return result
            } else {
                self.last_overhead_counts.increase(index as i32 - self.last_overhead_counts.index() as i32);
                return self.last_overhead_counts.clone()
            }
        }
    }

    /**
     * Returns the likelihood index given the allele counts.
     *
     * @param alleleCountArray the query allele counts. This must follow the format returned by
     *  {@link GenotypeAlleleCounts#copyAlleleCounts} with 0 offset.
     *
     * @throws IllegalArgumentException if {@code alleleCountArray} is not a valid {@code allele count array}:
     *  <ul>
     *      <li>is {@code null},</li>
     *      <li>or its length is not even,</li>
     *      <li>or it contains any negatives,
     *      <li>or the count sum does not match the calculator ploidy,</li>
     *      <li>or any of the alleles therein is negative or greater than the maximum allele index.</li>
     *  </ul>
     *
     * @return 0 or greater but less than {@link #genotypeCount}.
     */
    pub fn allele_counts_to_index(&mut self, allele_count_array: &[usize]) -> usize {
        if allele_count_array.len() % 2 != 0 {
            panic!("The allele counts array cannot have odd length")
        }
        self.allele_heap.clear();
        for i in (0..allele_count_array.len()).step_by(2) {
            let index = allele_count_array[i];
            let count = allele_count_array[i + 1];
            if count < 0 {
                panic!("No allele count can be less than 0")
            }
            for _ in (0..count).into_iter() {
                self.allele_heap.push(index)
            }
        }

        return self.allele_heap_to_index()
    }

    /**
     * Transforms the content of the heap into an index.
     *
     * <p>
     *     The heap contents are flushed as a result, so is left ready for another use.
     * </p>
     *
     * @return a valid likelihood index.
     */
    fn allele_heap_to_index(&mut self) -> usize {
        if self.allele_heap.len() != self.ploidy {
            panic!("The sum of allele counts must be equal to the ploidy of the calculator");
        }

        if self.allele_heap.peek().unwrap() >= &self.allele_count {
            panic!("Invalid allele {:?} more than the maximum {}",
                   self.allele_heap.peek(), self.allele_count - 1)
        }

        let mut result = 0;
        for p in (1..self.ploidy + 1).into_iter().rev() {
            let allele = self.allele_heap.pop().unwrap();
            if allele < 0 {
                panic!("Invalid allele {} must be >= 0", allele)
            }
            result += self.allele_first_genotype_offset_by_ploidy[[p, allele]]
        }
        return result as usize
    }
}