use rust_htslib::bam::record::{CigarStringView, Cigar, CigarString};
use utils::smith_waterman_aligner::SmithWatermanAligner;
use bio::alignment::pairwise::{Scoring, MIN_SCORE};
use bio_types::alignment::Alignment;

lazy_static! {
    static SW_PAD: String = format!("NNNNNNNNNN");
    // FROM GATK COMMENTS:
    // used in the bubble state machine to apply Smith-Waterman to the bubble sequence
    // these values were chosen via optimization against the NA12878 knowledge base
    static NEW_SW_PARAMETERS: Scoring = Scoring::new(-260, -11, 200, -150).xclip(MIN_SCORE).yclip(MIN_SCORE);
    // FROM GATK COMMENTS:
    // In Mutect2 and HaplotypeCaller reads are realigned to their *best* haplotypes, which is very different from a generic alignment.
    // The {@code NEW_SW_PARAMETERS} penalize a substitution error more than an indel up to a length of 9 bases!
    // Suppose, for example, that a read has a single substitution error, say C -> T, on its last base.  Those parameters
    // would prefer to extend a deletion until the next T on the reference is found in order to avoid the substitution, which is absurd.
    // Since these parameters are for aligning a read to the biological sequence we believe it comes from, the parameters
    // we choose should correspond to sequencer error.  They *do not* have anything to do with the prevalence of true variation!
    static ALIGNMENT_TO_BEST_HAPLOTYPE_SW_PARAMETERS: Scoring = Scoring::new(-30, -5, 10, -15).xclip(MIN_SCORE).yclip(MIN_SCORE);
}

pub struct CigarUtils {}

impl CigarUtils {


    pub fn cigar_consumes_read_bases(cig: &Cigar) -> bool {
        // Consumes read bases
        match cig {
            Cigar::Match(_)
            | Cigar::Equal(_)
            | Cigar::Diff(_)
            | Cigar::Ins(_)
            | Cigar::SoftClip(_) => true,
            _ => false
        }
    }

    pub fn cigar_consumes_reference_bases(cig: &Cigar) -> bool {
        // consumes reference bases
        match cig {
            Cigar::Match(_)
            | Cigar::Del(_)
            | Cigar::RefSkip(_)
            | Cigar::Equal(_)
            | Cigar::Diff(_) => true,
            _ => false
        }
    }

    pub fn cigar_is_soft_clip(cig: &Cigar) -> bool {
        match cig {
            Cigar::SoftClip(_) => true,
            _ => false,
        }
    }

    /**
     * Given a cigar string, soft clip up to leftClipEnd and soft clip starting at rightClipBegin
     * @param start initial index to clip within read bases, inclusive
     * @param stop final index to clip within read bases exclusive
     * @param clippingOperator      type of clipping -- must be either hard clip or soft clip
     */
    pub fn clip_cigar(cigar: &CigarStringView, start: u32, stop: u32, clipping_operator: Cigar) -> CigarString {
        let clip_left = start == 0;

        let mut new_cigar = Vec::new();

        let mut element_start = 0;
        for element in cigar.iter() {
            match element {
                // copy hard clips
                Cigar::HardClip(len) => {
                    new_cigar.push(Cigar::HardClip(*len))
                },
                Cigar::SoftClip(len)
                | Cigar::Diff(len)
                | Cigar::Equal(len)
                | Cigar::RefSkip(len)
                | Cigar::Del(len)
                | Cigar::Match(len)
                | Cigar::Ins(len)
                | Cigar::Pad(len) => {
                    let element_end = element_start + if CigarUtils::cigar_consumes_read_bases(element) { *len } else { 0 };

                    // element precedes start or follows end of clip, copy it to new cigar
                    if element_end <= start || element_start >= stop {
                        // edge case: deletions at edge of clipping are meaningless and we skip them
                        if CigarUtils::cigar_consumes_read_bases(element) ||
                            (element_start != start && element_start != stop) {
                            new_cigar.push(element.clone())
                        }
                    } else { // otherwise, some or all of the element is soft-clipped
                        let unclipped_length = if clip_left { element_end.checked_sub(stop) } else { start.checked_sub(element_start) };
                        match unclipped_length {
                            None => {
                                // Totally clipped
                                if CigarUtils::cigar_consumes_read_bases(element) {
                                    new_cigar.push(element.clone())
                                }
                            },
                            Some(unclipped_length) => {
                                let clipped_length = len.checked_sub(unclipped_length).unwrap();
                                if clip_left {
                                    new_cigar.push(CigarUtils::cigar_from_element_and_length(&clipping_operator, clipped_length));
                                    new_cigar.push(CigarUtils::cigar_from_element_and_length(element, unclipped_length));
                                } else {
                                    new_cigar.push(CigarUtils::cigar_from_element_and_length(element, unclipped_length));
                                    new_cigar.push(CigarUtils::cigar_from_element_and_length(&clipping_operator, clipped_length));
                                }
                            }
                        }
                    };
                    element_start = element_end
                }
            }
        }
        return CigarString(new_cigar)
    }

    /**
     * replace soft clips (S) with match (M) operators, normalizing the result by all the transformations of the {@link CigarBuilder} class:
     * merging consecutive identical operators and removing zero-length elements.  For example 10S10M -> 20M and 10S10M10I10I -> 20M20I.
     */
    pub fn revert_soft_clips(cigar: &CigarStringView) -> CigarString {
        let mut builder = Vec::new();
        for element in cigar.iter() {
            match element {
                Cigar::SoftClip(length) => {
                    builder.push(CigarUtils::cigar_from_element_and_length(&Cigar::Match(0), *length))
                },
                _ => {
                    builder.push(element.clone())
                }
            }
        }
        CigarString::from(builder)
    }

    /**
     * How many bases to the right does a read's alignment start shift given its cigar and the number of left soft clips
     */
    pub fn alignment_start_shift(cigar: &CigarStringView, num_clipped: i64) -> i64 {
        let ref_bases_clipped = 0;

        let element_start = 0; // this and elementEnd are indices in the read's bases
        for element in cigar.iter() {
            match element {
                // copy hard clips
                Cigar::HardClip(len) => {
                    continue
                },
                Cigar::SoftClip(len)
                | Cigar::Diff(len)
                | Cigar::Equal(len)
                | Cigar::RefSkip(len)
                | Cigar::Del(len)
                | Cigar::Match(len)
                | Cigar::Ins(len)
                | Cigar::Pad(len) => {
                    let element_end = element_start + if CigarUtils::cigar_consumes_read_bases(element) { *len as i64 } else { 0 };

                    if element_end <= num_clipped { // totally within clipped span -- this includes deletions immediately following clipping
                        ref_bases_clipped += if CigarUtils::cigar_consumes_reference_bases(element) { *len as i64 } else { 0 };
                    } else if element_start < num_clipped { // clip in middle of element, which means the element necessarily consumes read bases
                        let clipped_length = num_clipped - element_start;
                        ref_bases_clipped += if CigarUtils::cigar_consumes_reference_bases(element) { clipped_length } else { 0 };
                    }
                    element_start = element_end;
                }
            }
        }
        return ref_bases_clipped
    }

    pub fn cigar_from_element_and_length(cigar: &Cigar, length: u32) -> Cigar {
        match cigar {
            Cigar::Pad(_) => {
                return Cigar::Pad(length)
            },
            Cigar::Ins(_) => {
                return Cigar::Ins(length)
            },
            Cigar::Match(_) => {
                return Cigar::Match(length)
            },
            Cigar::Del(_) => {
                return Cigar::Del(length)
            },
            Cigar::RefSkip(_) => {
                return Cigar::RefSkip(length)
            },
            Cigar::Equal(_) => {
                return Cigar::Equal(length)
            },
            Cigar::Diff(_) => {
                return Cigar::Diff(length)
            },
            Cigar::SoftClip(_) => {
                return Cigar::SoftClip(length)
            },
            Cigar::HardClip(_) => {
                return Cigar::HardClip(length)
            }
        }
    }

    /**
     * Calculate the cigar elements for this path against the reference sequence.
     *
     * This assumes that the reference and alt sequence are haplotypes derived from a de Bruijn graph or SeqGraph and have the same
     * ref source and ref sink vertices.  That is, the alt sequence start and end are assumed anchored to the reference start and end, which
     * occur at the ends of the padded assembly region.  Hence, unlike read alignment, there is no concept of a start or end coordinate here.
     * Furthermore, it is important to note that in the rare case that the alt cigar begins or ends with a deletion, we must keep the leading
     * or trailing deletion in order to maintain the original reference span of the alt haplotype.  This can occur, for example, when the ref
     * haplotype starts with N repeats of a long sequence and the alt haplotype starts with N-1 repeats.
     *
     * @param aligner
     * @param refSeq the reference sequence that all of the bases in this path should align to
     * @return a Cigar mapping this path to refSeq, or null if no reasonable alignment could be found
     */
    pub fn calculate_cigar(ref_seq: &[u8], alt_seq: &[u8], aligner: SmithWatermanAligner) -> Option<CigarString> {
        if alt_seq.len() == 0 {
            // horrible edge case from the unit tests, where this path has no bases
            return CigarString::from(vec![Cigar::Del(ref_seq.len())])
        }

        //Note: this is a performance optimization.
        // If two strings are equal (a O(n) check) then it's trivial to get CIGAR for them.
        // Furthermore, if their lengths are equal and their element-by-element comparison yields two or fewer mismatches
        // it's also a trivial M-only CIGAR, because in order to have equal length one would need at least one insertion and
        // one deletion, in which case two substitutions is a better alignment.
        if alt_seq.len() == ref_seq.len() {
            let mismatch_count = (0..ref_seq.len()).into_par_iter().map(|n| {
                if alt_seq[n] == ref_seq[n] {
                    0
                } else {
                    1
                }
            }).sum::<usize>();

            if mismatch_count <= 2 {
                let matching = CigarString::from(vec![Cigar::Match(ref_seq.len())]);
                return matching
            }
        }

        let mut non_standard;
        let padded_ref = format!("{}{}{}", *SW_PAD, std::str::from_utf8(ref_seq).unwrap(), SW_PAD);
        let padded_path = format!("{}{}{}", *SW_PAD, std::str::from_utf8(alt_seq).unwrap(), SW_PAD);
        let alignment = aligner.align(ref_seq, alt_seq, *NEW_SW_PARAMETERS);

        if Self::is_s_w_failure(&alignment) {
            return None
        }

        // cut off the padding bases
        let base_start = *SW_PAD.len();
        let base_end = padded_path.len() - *SW_PAD.len() - 1; // -1 because it's inclusive not sure about this?


    }

    /**
     * Make sure that the SW didn't fail in some terrible way, and throw exception if it did
     */
    fn is_s_w_failure(alignment: &Alignment) -> bool {
        // check that the alignment starts at the first base, which it should given the padding
        if alignment.xstart != 0 || alignment.ystart != 0 {
            return true
        }

        // check that we aren't getting any S operators (which would be very bad downstream)
        for ce in CigarString::from_alignment(alignment, false).iter() {
            match ce {
                Cigar::SoftClip(_) => {
                    return true
                },
                _ => {
                    continue
                }
            }
        }

        return false
    }
}