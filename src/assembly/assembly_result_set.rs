use hashlink::LinkedHashSet;
use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::mem::swap;

use crate::assembly::assembly_region::AssemblyRegion;
use crate::assembly::assembly_result::AssemblyResult;
use crate::haplotype::event_map::EventMap;
use crate::haplotype::haplotype::Haplotype;
use crate::model::byte_array_allele::Allele;
use crate::model::variant_context::VariantContext;
use crate::read_threading::abstract_read_threading_graph::AbstractReadThreadingGraph;
use crate::reads::bird_tool_reads::BirdToolRead;
use crate::utils::errors::BirdToolError;
use crate::utils::simple_interval::{Locatable, SimpleInterval};

/**
 * Collection of read assembly using several kmerSizes.
 *
 * <p>
 *     There could be a different assembly per each kmerSize. In turn, haplotypes are result of one of those
 *     assemblies.
 * </p>
 *
 * <p>
 *     Where there is more than one possible kmerSize that generates a haplotype we consider the smaller one.
 * </p>
 *
 * @original_author Valentin Ruano-Rubio &lt;valentin@broadinstitute.com&gt;
 * @author Rhys Newell; rhys.newell@hdr.qut.edu.au; Rust re-implementation
 */
#[derive(Debug)]
pub struct AssemblyResultSet<A: AbstractReadThreadingGraph> {
    // kmer size and assembly_results index hashmap
    pub(crate) assembly_result_by_kmer_size: HashMap<usize, usize>,
    pub(crate) haplotypes: LinkedHashSet<Haplotype<SimpleInterval>>,
    // haplotype and assembly_results index hashmap
    pub(crate) assembly_result_by_haplotype: HashMap<Haplotype<SimpleInterval>, usize>,
    pub(crate) region_for_genotyping: AssemblyRegion,
    pub(crate) full_reference_with_padding: Vec<u8>,
    pub(crate) padded_reference_loc: SimpleInterval,
    pub(crate) variation_present: bool,
    pub(crate) ref_haplotype: Haplotype<SimpleInterval>,
    pub(crate) kmer_sizes: BTreeSet<usize>,
    pub(crate) variation_events: BTreeSet<VariantContext>,
    pub(crate) last_max_mnp_distance_used: Option<usize>,
    pub(crate) assembly_results: Vec<AssemblyResult<SimpleInterval, A>>,
}

impl<A: AbstractReadThreadingGraph> AssemblyResultSet<A> {
    /**
     * Constructs a new empty assembly result set.
     */
    pub fn new(
        assembly_region: AssemblyRegion,
        full_reference_with_padding: Vec<u8>,
        ref_loc: SimpleInterval,
        ref_haplotype: Haplotype<SimpleInterval>,
    ) -> Self {
        let mut haplotypes = LinkedHashSet::new();
        haplotypes.insert(ref_haplotype.clone());

        Self {
            assembly_result_by_kmer_size: HashMap::new(),
            haplotypes,
            assembly_result_by_haplotype: HashMap::new(),
            region_for_genotyping: assembly_region,
            full_reference_with_padding,
            padded_reference_loc: ref_loc,
            variation_present: false,
            ref_haplotype,
            kmer_sizes: BTreeSet::new(),
            variation_events: BTreeSet::new(),
            last_max_mnp_distance_used: None,
            assembly_results: Vec::new(),
        }
    }

    pub fn default() -> Self {
        Self {
            assembly_result_by_kmer_size: HashMap::new(),
            haplotypes: LinkedHashSet::new(),
            assembly_result_by_haplotype: HashMap::new(),
            region_for_genotyping: AssemblyRegion::new(
                SimpleInterval::new(0, 0, 1),
                false,
                0,
                1,
                0,
                0,
                0.0
            ),
            full_reference_with_padding: Vec::new(),
            padded_reference_loc: SimpleInterval::new(0, 0, 1),
            variation_present: false,
            ref_haplotype: Haplotype::new("A".as_bytes(), false),
            kmer_sizes: BTreeSet::new(),
            variation_events: BTreeSet::new(),
            last_max_mnp_distance_used: None,
            assembly_results: Vec::new(),
        }
    }

    pub fn get_ref_haplotype(&self) -> &Haplotype<SimpleInterval> {
        &self.ref_haplotype
    }

    /// Adds a haplotype to the result set without indicating a generating assembly result.
    ///
    ///
    /// It is possible to call this method with the same haplotype several times. In that the second and further
    /// calls won't have any effect (thus returning `false`).
    ///
    ///
    /// @param `h` the haplotype to add to the assembly result set.
    ///
    /// @return `true` if the assembly result set has been modified as a result of this call.
    ///
    pub fn add_haplotype(&mut self, h: Haplotype<SimpleInterval>) -> bool {
        if self.haplotypes.contains(&h) {
            return false;
        } else {
            // debug!("Loc {:?} hap {:?}", &self.padded_reference_loc, &h);
            self.update_reference_haplotype(&h);
            self.haplotypes.insert(h);
            return true;
        }
    }

    /**
     * Given whether a new haplotype that has been already added to {@link #haplotypes} collection is the
     * reference haplotype and updates {@link #refHaplotype} accordingly.
     *
     * <p>
     *     This method assumes that the colling code has verified that the haplotype was not already in {@link #haplotypes}
     *     I.e. that it is really a new one. Otherwise it will result in an exception if it happen to be a reference
     *     haplotype and this has already be set. This is the case even if the new haplotypes and the current reference
     *     are equal.
     * </p>
     *
     * @param `new_haplotype` the new haplotype.
     */
    fn update_reference_haplotype(&mut self, new_haplotype: &Haplotype<SimpleInterval>) {
        if new_haplotype.is_reference() {
            if self.ref_haplotype.is_no_call() {
                self.ref_haplotype = new_haplotype.clone();
            } else if &self.ref_haplotype != new_haplotype {
                panic!("The assembly result set already has a reference that is different to this haplotype")
            }
        };
    }

    pub fn get_haplotype_list(&self) -> Vec<Haplotype<SimpleInterval>> {
        return self
            .haplotypes
            .iter()
            .cloned()
            .collect::<Vec<Haplotype<SimpleInterval>>>();
    }

    pub fn get_haplotypes(&self) -> &LinkedHashSet<Haplotype<SimpleInterval>> {
        return &self.haplotypes;
    }

    /**
     * Adds simultaneously a haplotype and the generating assembly-result.
     *
     * <p>
     *     Haplotypes and their assembly-result can be added multiple times although just the first call will have
     *     any effect (return value is {@code true}).
     * </p>
     *
     *
     * @param h haplotype to add.
     * @param ar assembly-result that is assumed to have given rise to that haplotype.
     *
     *
     * @return `true` iff this called changes the assembly result set.
     */
    pub fn add_haplotype_and_assembly_result(
        &mut self,
        h: Haplotype<SimpleInterval>,
        ar: usize,
    ) -> bool {
        let assembly_result_addition_return = (self.assembly_results.len() - 1) <= ar;
        if self.haplotypes.contains(&h) {
            let previous_ar = self.assembly_result_by_haplotype.get(&h);
            if previous_ar.is_none() {
                self.assembly_result_by_haplotype.insert(h, ar);
                true
            } else if assembly_result_addition_return {
                if self.assembly_results[*previous_ar.unwrap()].discovered_haplotypes
                    != self.assembly_results[ar].discovered_haplotypes
                {
                    panic!("There is already a different assembly result for the input haplotype")
                } else {
                    assembly_result_addition_return
                }
            } else {
                assembly_result_addition_return
            }
        } else {
            if !h.allele.is_ref {
                self.variation_present = true;
            };
            self.haplotypes.insert(h.clone());
            self.assembly_result_by_haplotype.insert(h, ar);
            true
        }
    }

    /**
     * Add a assembly-result object.
     *
     * @param ar the assembly result to add.
     *
     * @throws NullPointerException if {@code ar} is {@code null}.
     * @throws IllegalStateException if there is an assembly result with the same kmerSize.
     * @return {@code usize} return index in assembly result array that this assembly result belongs
     */
    pub fn add_assembly_result(&mut self, ar: AssemblyResult<SimpleInterval, A>) -> usize {
        let kmer_size = ar.get_kmer_size();

        if self.assembly_result_by_kmer_size.contains_key(&kmer_size) {
            if ar
                .ne(&self.assembly_results
                    [*self.assembly_result_by_kmer_size.get(&kmer_size).unwrap()])
            {
                panic!("a different assembly result with the same kmerSize was already added");
            } else {
                let ar_ind = *self.assembly_result_by_kmer_size.get(&kmer_size).unwrap();
                if ar.discovered_haplotypes.len() > 0 {
                    for hap in ar.discovered_haplotypes.into_iter() {
                        self.add_haplotype_and_assembly_result(hap, ar_ind);
                    }
                }

                *self.assembly_result_by_kmer_size.get(&kmer_size).unwrap()
            }
        } else {
            self.assembly_results.push(ar);
            let ar_ind = self.assembly_results.len() - 1;

            self.assembly_result_by_kmer_size.insert(kmer_size, ar_ind);
            self.kmer_sizes.insert(kmer_size);

            if self
                .assembly_results
                .last()
                .unwrap()
                .discovered_haplotypes
                .len()
                > 0
            {
                for hap in self
                    .assembly_results
                    .last()
                    .unwrap()
                    .discovered_haplotypes
                    .clone()
                    .into_iter()
                {
                    self.add_haplotype_and_assembly_result(hap, ar_ind);
                }
            }

            ar_ind
        }
    }

    /**
     * Returns a sorted set of variant events that best explain the haplotypes found by the assembly
     * across kmerSizes.
     *
     * <p/>
     * The result is sorted incrementally by location.
     * @param maxMnpDistance Phased substitutions separated by this distance or less are merged into MNPs.  More than
     *                       two substitutions occuring in the same alignment block (ie the same M/X/EQ CIGAR element)
     *                       are merged until a substitution is separated from the previous one by a greater distance.
     *                       That is, if maxMnpDistance = 1, substitutions at 10,11,12,14,15,17 are partitioned into a MNP
     *                       at 10-12, a MNP at 14-15, and a SNP at 17.  May not be negative.
     * @return never {@code null}, but perhaps an empty collection.
     */
    pub fn get_variation_events(
        &mut self,
        max_mnp_distance: usize,
    ) -> Result<BTreeSet<VariantContext>, BirdToolError> {
        let same_mnp_distance = if self.last_max_mnp_distance_used.is_some() {
            if &max_mnp_distance == self.last_max_mnp_distance_used.as_ref().unwrap() {
                true
            } else {
                false
            }
        } else {
            false
        };
        self.last_max_mnp_distance_used = Some(max_mnp_distance);

        if self.variation_events.is_empty()
            || !same_mnp_distance
            || self
                .haplotypes
                .iter()
                .any(|hap| !hap.allele.is_ref && hap.event_map.is_none())
        {
            match self.regenerate_variation_events(max_mnp_distance) {
                Ok(_) => {
                    // pass
                }
                Err(error) => return Err(error),
            }
        }

        return Ok(self.variation_events.clone());
    }

    pub fn regenerate_variation_events(
        &mut self,
        max_mnp_distance: usize,
    ) -> Result<(), BirdToolError> {
        let mut haplotype_list = LinkedHashSet::new();
        swap(&mut self.haplotypes, &mut haplotype_list);

        let mut haplotype_list = haplotype_list
            .into_iter()
            .collect::<Vec<Haplotype<SimpleInterval>>>();

        match EventMap::build_event_maps_for_haplotypes(
            &mut haplotype_list,
            self.full_reference_with_padding.as_slice(),
            &self.padded_reference_loc,
            max_mnp_distance,
        ) {
            Ok(_) => {
                // pass
            }
            Err(error) => return Err(error),
        }
        self.variation_events = self.get_all_variant_contexts(&haplotype_list);
        self.last_max_mnp_distance_used = Some(max_mnp_distance);
        self.variation_present = haplotype_list.iter().any(|h| !h.allele.is_ref);
        self.haplotypes = haplotype_list
            .into_iter()
            .collect::<LinkedHashSet<Haplotype<SimpleInterval>>>();
        Ok(())
    }

    /**
     * Get all of the VariantContexts in the event maps for all haplotypes, sorted by their start position and then arbitrarily by indel length followed by bases
     * @param haplotypes the set of haplotypes to grab the VCs from
     * @return a sorted set of variant contexts
     */
    fn get_all_variant_contexts<'a, I, L: 'a + Locatable>(
        &self,
        haplotypes: I,
    ) -> BTreeSet<VariantContext>
    where
        I: IntoIterator<Item = &'a Haplotype<L>>,
    {
        // Using the cigar from each called haplotype figure out what events need to be written out in a VCF file
        let vcs = haplotypes
            .into_iter()
            .flat_map(|h| h.event_map.as_ref().unwrap().map.values().cloned())
            .collect::<BTreeSet<VariantContext>>();

        return vcs;
    }

    pub fn remove_all(mut self, reads: &Vec<BirdToolRead>) -> Self {
        // debug!(
        //     "remove {} of {} reads",
        //     reads.len(),
        //     self.region_for_genotyping.reads.len()
        // );
        self.region_for_genotyping = self.region_for_genotyping.remove_all(reads);
        return self;
    }

    // /**
    //  * Determines the VariantContext type for each haplotype
    //  */
    // pub fn determine_type(&mut self) {
    //     self.haplotypes.iter().for_each(|h| {
    //         h.
    //     })
    // }

    /**
     * Trims an assembly result set down based on a new set of trimmed haplotypes.
     *
     * @param trimmedAssemblyRegion the trimmed down active region.
     *
     * @throws NullPointerException if any argument in {@code null} or
     *      if there are {@code null} entries in {@code originalByTrimmedHaplotypes} for trimmed haplotype keys.
     * @throws IllegalArgumentException if there is no reference haplotype amongst the trimmed ones.
     *
     * @return never {@code null}, a new trimmed assembly result set.
     */
    pub fn trim_to(mut self, trimmed_assembly_region: AssemblyRegion) -> Self {
        let original_by_trimmed_haplotypes =
            self.calculate_original_by_trimmed_haplotypes(&trimmed_assembly_region.padded_span);

        // debug!(
        //     "Original by trimmed haplotypes {:?}",
        //     &original_by_trimmed_haplotypes
        // );
        let mut new_assembly_result_by_haplotype = HashMap::new();
        let mut new_haplotypes = LinkedHashSet::new();

        for (trimmed, original) in original_by_trimmed_haplotypes {
            let ass = self.assembly_result_by_haplotype.get(&original);
            if trimmed.is_ref() {
                self.ref_haplotype = trimmed.clone()
            }
            match ass {
                None => {
                    if !trimmed.is_ref() {
                        self.variation_present = true;
                    }
                    new_haplotypes.insert(trimmed);
                }
                Some(ass) => {
                    if !trimmed.is_ref() {
                        self.variation_present = true;
                    }
                    new_haplotypes.insert(trimmed.clone());
                    new_assembly_result_by_haplotype.insert(trimmed, *ass);
                }
            };
        }

        // trimmed_assembly_region.reads = self.region_for_genotyping.reads;
        self.region_for_genotyping = trimmed_assembly_region;
        self.haplotypes.clear();
        self.assembly_result_by_haplotype.clear();
        self.haplotypes = new_haplotypes;
        self.assembly_result_by_haplotype = new_assembly_result_by_haplotype;
        // self.variation_present = self.haplotypes.iter().any(|hap| !hap.is_ref());

        return self;
    }

    fn calculate_original_by_trimmed_haplotypes<'b>(
        &'b mut self,
        span: &SimpleInterval,
    ) -> BTreeMap<Haplotype<SimpleInterval>, Haplotype<SimpleInterval>> {
        // debug!(
        //     "Trimming active region {:?} {} reads with {} hapotypes -> cigar 1 {}",
        //     &self.region_for_genotyping.active_span,
        //     &self.region_for_genotyping.reads.len(),
        //     self.haplotypes.len(),
        //     self.haplotypes.iter().next().unwrap().cigar.to_string()
        // );
        let haplotype_list = self
            .haplotypes
            .iter()
            .cloned()
            .collect::<Vec<Haplotype<SimpleInterval>>>();
        // trim down the haplotypes
        let sorted_original_by_trimmed_haplotypes =
            Self::trim_down_haplotypes(span, haplotype_list);
        //TODO: Check these are sorted by size and bases
        return sorted_original_by_trimmed_haplotypes;
    }

    fn trim_down_haplotypes(
        span: &SimpleInterval,
        haplotype_list: Vec<Haplotype<SimpleInterval>>,
    ) -> BTreeMap<Haplotype<SimpleInterval>, Haplotype<SimpleInterval>> {
        let mut original_by_trimmed_haplotypes = BTreeMap::new();

        for h in haplotype_list {
            let trimmed = h.trim(span.clone());

            match trimmed {
                Err(_) => panic!("Unhandled Trimming error"),
                Ok(trimmed) => match trimmed {
                    Some(trimmed) => {
                        if original_by_trimmed_haplotypes.contains_key(&trimmed) {
                            if trimmed.is_ref() {
                                original_by_trimmed_haplotypes.remove(&trimmed);
                                original_by_trimmed_haplotypes.insert(trimmed, h);
                            }
                        } else {
                            original_by_trimmed_haplotypes.insert(trimmed, h);
                        };
                    }
                    None => {
                        if h.is_ref() {
                            panic!("Trimming eliminated the reference haplotype");
                        };
                    //     debug!("Throwing out haplotype {:?} with cigar {:?} becuase it starts with or ends \
                    // with an insertion or deletion when trimmed to {:?}", &h, &h.cigar, span);
                    }
                },
            }
        }

        return original_by_trimmed_haplotypes;
    }

    // fn map_original_to_trimmed(
    //     original_by_trimmed_haplotypes: HashMap<
    //         Haplotype<SimpleInterval>,
    //         Haplotype<SimpleInterval>,
    //     >,
    //     trimmed_haplotypes: Vec<Haplotype<SimpleInterval>>,
    // ) -> BTreeMap<Haplotype<SimpleInterval>, Haplotype<SimpleInterval>> {
    //     let mut sorted_original_by_trimmed_haplotypes = BTreeMap::new();

    //     for trimmed in trimmed_haplotypes {
    //         let value = original_by_trimmed_haplotypes
    //             .get(&trimmed)
    //             .unwrap()
    //             .clone();
    //         sorted_original_by_trimmed_haplotypes.insert(trimmed, value);
    //     }

    //     return sorted_original_by_trimmed_haplotypes;
    // }
}
