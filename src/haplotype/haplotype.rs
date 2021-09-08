use haplotype::event_map::EventMap;
use model::byte_array_allele::{Allele, ByteArrayAllele};
use ordered_float::OrderedFloat;
use reads::alignment_utils::AlignmentUtils;
use reads::cigar_builder::CigarBuilder;
use reads::cigar_utils::CigarUtils;
use rust_htslib::bam::record::{Cigar, CigarString, CigarStringView};
use std::cmp::Ordering;
use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};
use utils::errors::BirdToolError;
use utils::simple_interval::{Locatable, SimpleInterval};

// lazy_static! {
//     pub static ref SIZE_AND_BASE_ORDER: Then<Extract<Fn(&Haplotype<Locatable>)>>
// }
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Haplotype<'a, L: Locatable> {
    pub(crate) allele: ByteArrayAllele,
    pub(crate) genome_location: Option<L>,
    pub(crate) event_map: Option<EventMap<'a>>,
    pub(crate) cigar: CigarString,
    pub(crate) alignment_start_hap_wrt_ref: usize,
    pub(crate) score: OrderedFloat<f64>,
    // debug information for tracking kmer sizes used in graph construction for debug output
    pub(crate) kmer_size: usize,
}

impl<'a, L: Locatable> Haplotype<'a, L> {
    /**
     * Main constructor
     *
     * @param bases a non-null array of bases
     * @param isRef is this the reference haplotype?
     */
    pub fn new(bases: &[u8], is_ref: bool) -> Haplotype<'a, L> {
        Haplotype {
            allele: ByteArrayAllele::new(bases, is_ref),
            genome_location: None,
            event_map: None,
            cigar: CigarString::from(vec![Cigar::Match(0)]),
            alignment_start_hap_wrt_ref: 0,
            score: OrderedFloat(std::f64::MIN),
            kmer_size: 0,
        }
    }

    pub fn set_alignment_start_hap_wrt_ref(&mut self, value: usize) {
        self.alignment_start_hap_wrt_ref = value
    }

    pub fn hash_code(&self) -> u64 {
        let mut hasher = DefaultHasher::new();
        self.get_bases().hash(&mut hasher);
        hasher.finish()
    }

    // pub fn get_bases(&self) -> &[u8] {
    //     &self.allele.bases[..]
    // }

    pub fn get_start_position(&self) -> usize {
        self.genome_location.as_ref().unwrap().get_start()
    }

    pub fn get_cigar(&self) -> &CigarString {
        &self.cigar
    }

    pub fn get_stop_position(&self) -> usize {
        self.genome_location.as_ref().unwrap().get_end()
    }

    pub fn get_alignment_start_hap_wrt_ref(&self) -> usize {
        self.alignment_start_hap_wrt_ref
    }

    pub fn set_cigar(&mut self, cigar_string: Vec<Cigar>) {
        self.cigar = CigarString::from(cigar_string)
    }

    pub fn set_genome_location(&mut self, genome_location: L) {
        self.genome_location = Some(genome_location)
    }

    pub fn len(&self) -> usize {
        self.allele.len()
    }

    /**
     * Create a new Haplotype derived from this one that exactly spans the provided location
     *
     * Note that this haplotype must have a contain a genome loc for this operation to be successful.  If no
     * GenomeLoc is contained than @throws an IllegalStateException
     *
     * Also loc must be fully contained within this Haplotype's genomeLoc.  If not an IllegalArgumentException is
     * thrown.
     *
     * @param loc a location completely contained within this Haplotype's location
     * @return a new Haplotype within only the bases spanning the provided location, or null for some reason the haplotype would be malformed if
     */
    pub fn trim<'b>(
        &'b self,
        loc: SimpleInterval,
    ) -> Result<Option<Haplotype<'a, SimpleInterval>>, BirdToolError> {
        if self.genome_location.is_none() {
            return Err(BirdToolError::InvalidLocation(format!(
                "Attempting to trim haplotype with no genome location"
            )));
        }

        if !self.genome_location.as_ref().unwrap().contains(&loc) {
            return Err(BirdToolError::InvalidLocation(format!(
                "Can only trim a Haplotype to a containing span."
            )));
        }

        let new_start = loc.get_start() - self.genome_location.as_ref().unwrap().get_start();
        let new_stop = new_start + loc.get_end() - loc.get_start();

        // note: the following returns null if the bases covering the ref interval start or end in a deletion.
        let new_bases = AlignmentUtils::get_bases_covering_ref_interval(
            new_start,
            new_stop,
            self.get_bases(),
            0,
            &self.cigar,
        );

        match new_bases {
            None => return Ok(None),
            Some(new_bases) => {
                if new_bases.len() == 0 {
                    return Ok(None);
                };

                // note: trimCigarByReference does not remove leading or trailing indels, while getBasesCoveringRefInterval does remove bases
                // of leading and trailing insertions.  We must remove leading and trailing insertions from the Cigar manually.
                // we keep leading and trailing deletions because these are necessary for haplotypes to maintain consistent reference coordinates
                let new_cigar = AlignmentUtils::trim_cigar_by_reference(
                    self.cigar.clone(),
                    new_start as u32,
                    new_stop as u32,
                )
                .cigar;
                let leading_insertion =
                    !CigarUtils::cigar_consumes_reference_bases(new_cigar.0.first().unwrap());
                let trailing_insertion =
                    !CigarUtils::cigar_consumes_reference_bases(new_cigar.0.last().unwrap());

                let first_index_to_keep_inclusive = if leading_insertion { 1 } else { 0 };
                let last_index_to_keep_exclusive =
                    new_cigar.0.len() - (if trailing_insertion { 1 } else { 0 });

                if last_index_to_keep_exclusive <= first_index_to_keep_inclusive {
                    // edge case of entire cigar is insertion
                    return Ok(None);
                };

                let leading_indel_trimmed_new_cigar = if !(leading_insertion || trailing_insertion)
                {
                    new_cigar
                } else {
                    let mut tmp = CigarBuilder::new(false);
                    tmp.add_all(
                        new_cigar.0[first_index_to_keep_inclusive..last_index_to_keep_exclusive]
                            .to_vec(),
                    );
                    match tmp.make(false) {
                        Ok(cigar_string) => cigar_string,
                        _ => {
                            return Err(BirdToolError::CigarBuilderError(format!(
                                "Cigar builder failed"
                            )))
                        }
                    }
                };

                let mut ret = Haplotype::new(new_bases, self.is_ref());
                ret.cigar = leading_indel_trimmed_new_cigar;
                ret.set_genome_location(loc);
                ret.score = self.score;
                ret.kmer_size = self.kmer_size;
                ret.alignment_start_hap_wrt_ref = new_start + self.alignment_start_hap_wrt_ref;

                return Ok(Some(ret));
            }
        }
    }

    pub fn is_ref(&self) -> bool {
        self.allele.is_ref
    }

    /**
     * Get the haplotype cigar extended by padSize M at the tail, consolidated into a clean cigar
     *
     * @param padSize how many additional Ms should be appended to the end of this cigar.  Must be >= 0
     * @return a newly allocated Cigar that consolidate(getCigar + padSize + M)
     */
    pub fn get_consolidated_padded_ciagr(
        &self,
        pad_size: usize,
    ) -> Result<CigarString, BirdToolError> {
        let mut builder = CigarBuilder::new(true);
        builder.add_all(self.cigar.0.clone());
        builder.add(Cigar::Match(pad_size as u32));
        return builder.make(false);
    }
}

impl<'a, L: Locatable> Hash for Haplotype<'a, L> {
    fn hash<H: Hasher>(&self, state: &mut H) {
        self.cigar.hash(state);
        self.allele.hash(state);
        self.genome_location.hash(state);
        self.score.hash(state);
        self.kmer_size.hash(state);
    }
}

impl<'a, L: Locatable> Ord for Haplotype<'a, L> {
    fn cmp(&self, other: &Self) -> Ordering {
        self.get_bases()
            .len()
            .cmp(&other.get_bases().len())
            .then_with(|| self.get_bases().cmp(other.get_bases()))
    }
}

impl<'a, L: Locatable> PartialOrd for Haplotype<'a, L> {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl<'a, L: Locatable> Allele for Haplotype<'a, L> {
    fn is_reference(&self) -> bool {
        self.allele.is_ref
    }

    fn length(&self) -> usize {
        self.allele.len()
    }

    fn is_symbolic(&self) -> bool {
        self.allele.is_symbolic
    }

    fn is_called(&self) -> bool {
        !self.allele.is_no_call
    }

    fn is_no_call(&self) -> bool {
        self.allele.is_no_call
    }

    fn get_bases(&self) -> &[u8] {
        self.allele.get_bases()
    }

    fn no_call() -> Self {
        Self {
            allele: ByteArrayAllele::no_call(),
            genome_location: None,
            event_map: None,
            cigar: CigarString::from(vec![Cigar::Match(0)]),
            alignment_start_hap_wrt_ref: 0,
            score: OrderedFloat(std::f64::MIN),
            kmer_size: 0,
        }
    }
}
