use rust_htslib::bam::Record;
use reads::bird_tool_reads::BirdToolRead;
use bio_types::sequence::SequenceRead;
use reads::read_utils::ReadUtils;
use reads::clipping_op::ClippingOp;

/**
 * A comprehensive clipping tool.
 *
 * General Contract:
 *  - All clipping operations return a new read with the clipped bases requested, it never modifies the original read.
 *  - If a read is fully clipped, return an empty SAMRecord, never null.
 *  - When hard clipping, add cigar operator H for every *reference base* removed (i.e. Matches, SoftClips and Deletions, but *not* insertions). See Hard Clipping notes for details.
 *
 *
 * There are several types of clipping to use:
 *
 * Write N's:
 *   Change the bases to N's in the desired region. This can be applied anywhere in the read.
 *
 * Write Q0's:
 *   Change the quality of the bases in the desired region to Q0. This can be applied anywhere in the read.
 *
 * Write both N's and Q0's:
 *   Same as the two independent operations, put together.
 *
 * Soft Clipping:
 *   Do not change the read, just mark the reads as soft clipped in the Cigar String
 *   and adjust the alignment start and end of the read.
 *
 * Hard Clipping:
 *   Creates a new read without the hard clipped bases (and base qualities). The cigar string
 *   will be updated with the cigar operator H for every reference base removed (i.e. Matches,
 *   Soft clipped bases and deletions, but *not* insertions). This contract with the cigar
 *   is necessary to allow read.getUnclippedStart() / End() to recover the original alignment
 *   of the read (before clipping).
 *
 */
pub struct ReadClipper {
    read: BirdToolRead,
    was_clipped: bool,
    ops: Vec<ClippingOp>
}

impl ReadClipper {

    pub fn new(read: BirdToolRead) -> ReadClipper {
        ReadClipper {
            read,
            was_clipped: false,
            ops: Vec::new(),
        }
    }

    /**
     * Hard clip the read to the variable region (from refStart to refStop)
     *
     * @param read     the read to be clipped
     * @param refStart the beginning of the variant region (inclusive)
     * @param refStop  the end of the variant region (inclusive)
     * @return the read hard clipped to the variant region (Could return an empty, unmapped read)
     */
    pub fn hard_clip_to_region(
        read: BirdToolRead, ref_start: usize, ref_stop: usize
    ) -> BirdToolRead {
        let start = read.get_start();
        let end = read.get_end();
        return Self::hard_clip_to_region_with_alignment_interval()
    }

    fn hard_clip_to_region_with_alignment_interval(
        read: BirdToolRead, ref_start: usize, ref_stop: usize, alignment_start: usize, alignment_stop: usize
    ) -> BirdToolRead {
        if alignment_start <= ref_stop && alignment_stop >= ref_start {
            if alignment_start < ref_start && alignment_stop > ref_stop {
                return ReadClipper::new(read).hardref_start.checked_sub(1).unwrap_or(0), r).
            }
        }
    }

    /**
     * Generic functionality to  clip a read, used internally by hardClipByReferenceCoordinatesLeftTail
     * and hardClipByReferenceCoordinatesRightTail. Should not be used directly.
     *
     * Note, it REQUIRES you to give the directionality of your hard clip (i.e. whether you're clipping the
     * left of right tail) by specifying either refStart == None or refStop == None.
     *
     * @param refStart  first base to clip (inclusive)
     * @param refStop last base to clip (inclusive)
     * @param clippingOp clipping operation to be performed
     * @return a new read, without the clipped bases (May return empty, unclipped reads)
     */
    fn clip_by_reference_coordinates(
        &mut self, ref_start: Option<usize>, ref_stop: Option<usize>, clipping_op: ClippingRepresentation
    ) -> BirdToolRead {
        if self.read.read.is_empty() {
            return ReadUtils::empty_read(self.read)
        }
        if clipping_op == ClippingRepresentation::SoftclipBases && self.read.read.is_unmapped() {
            panic!("Cannot soft-clip read {:?} by reference coordinates because it is unmapped", self.read)
        }

        let mut start;
        let mut stop;

        // Determine the read coordinate to start and stop hard clipping
        match ref_start {
            None => {
                match ref_stop {
                    None => {
                        panic!("Only one of ref_start or ref_stop can be None, not both.")
                    },
                    Some(ref_stop) => {
                        start = Some(0);
                        let stop_pos_and_operator = ReadUtils::get_read_index_for_reference_coordinate_from_read(&self.read, ref_stop);
                        match stop_pos_and_operator.0 {
                            Some(pos) => {
                                stop = Some(pos - (if ReadUtils::cigar_consumes_read_bases(&stop_pos_and_operator.1.unwrap()) { 0 } else { 1 }));
                            },
                            None => {
                                stop = None;
                            }
                        }
                    }
                }
            },
            Some(ref_start) => {
                match ref_stop {
                    None => {
                        start = ReadUtils::get_read_index_for_reference_coordinate_from_read(&self.read, ref_start).0;
                        stop = (self.read.read.seq_len() as usize).checked_sub(1);
                    },
                    Some(ref_stop) => {
                        panic!("Either ref_start or ref_stop needs to be None")
                    }
                }
            }
        }

        if start.is_none() || stop.is_none() {
            return self.read.clone()
        };

        if stop.unwrap_or(0) > self.read.read.len() - 1 {
            panic!("Trying to clip after the end of a read");
        };

        if stop.unwrap_or(0) < start.unwrap_or(0) {
            panic!("Start > Stop, this should never happen");
        };

        if start.unwrap_or(0) > 0 && stop.unwrap_or(0) < self.read.read.len() - 1 {
            panic!("Trying to clip the middle of a read");
        };

        self.add_op(ClippingOp::new(start.unwrap(), stop.unwrap()));

        let clipped_read = self.clip_read(clipping_op);
        self.ops = Vec::new();

        return clipped_read
    }

    /**
     * Hard clips both tails of a read.
     *   Left tail goes from the beginning to the 'left' coordinate (inclusive)
     *   Right tail goes from the 'right' coordinate (inclusive) until the end of the read
     *
     * @param left the coordinate of the last base to be clipped in the left tail (inclusive)
     * @param right the coordinate of the first base to be clipped in the right tail (inclusive)
     * @return a new read, without the clipped bases (Could return an empty, unmapped read)
     */
    fn hard_clip_both_ends_by_reference_coordinates(&mut self, left: usize, right: usize) -> BirdToolRead {
        if self.read.read.is_empty() || left == right {
            return ReadUtils::empty_read(&self.read)
        }

        let left_tail_read =
    }

    /**
     * Add clipping operation to the read.
     *
     * You can add as many operations as necessary to this read before clipping. Beware that the
     * order in which you add these operations matter. For example, if you hard clip the beginning
     * of a read first then try to hard clip the end, the indices will have changed. Make sure you
     * know what you're doing, otherwise just use the static functions below that take care of the
     * ordering for you.
     *
     * Note: You only choose the clipping mode when you use clipRead()
     *
     * @param op a ClippingOp object describing the area you want to clip.
     */
    pub fn add_op(&mut self, op: ClippingOp) {
        self.ops.push(op)
    }

    /**
     * Clips a read according to ops and the chosen algorithm.
     *
     * @param algorithm What mode of clipping do you want to apply for the stacked operations.
     * @return the read with the clipping applied (Could be an empty, unmapped read if the clip removed all bases)
     */
    pub fn clip_read(&mut self, algorithm: ClippingRepresentation) -> BirdToolRead {
        if self.ops.is_empty() {
            return self.read.clone()
        }

        let mut clipped_read = self.read.clone();

        for op in self.ops.iter_mut() {
            let read_length = clipped_read.read.len();
            //check if the clipped read can still be clipped in the range requested
            if op.start < read_length {
                if op.stop >= read_length {
                    op.stop = read_length - 1;
                }

                clipped_read = op.apply(algorithm, &clipped_read);
            }
        }

        self.was_clipped = true;
        self.ops.clear();
        if clipped_read.read.is_empty() {
            return ReadUtils::empty_read(&clipped_read)
        }

        return clipped_read
    }
}


/**
 * How should we represent a clipped bases in a read?
 */
#[derive(Debug, Clone, PartialOrd, PartialEq, Ord, Eq)]
pub enum ClippingRepresentation {
    /** Clipped bases are changed to Ns */
    WriteNs,

    /** Clipped bases are changed to have Q0 quality score */
    WriteQ0s,

    /** Clipped bases are change to have both an N base and a Q0 quality score */
    WriteNsQ0s,

    /**
     * Change the read's cigar string to soft clip (S, see sam-spec) away the bases.
     * Note that this can only be applied to cases where the clipped bases occur
     * at the start or end of a read.
     */
    SoftclipBases,

    /**
     * WARNING: THIS OPTION IS STILL UNDER DEVELOPMENT AND IS NOT SUPPORTED.
     *
     * Change the read's cigar string to hard clip (H, see sam-spec) away the bases.
     * Hard clipping, unlike soft clipping, actually removes bases from the read,
     * reducing the resulting file's size but introducing an irrevesible (i.e.,
     * lossy) operation.  Note that this can only be applied to cases where the clipped
     * bases occur at the start or end of a read.
     */
    HardclipBases,

    /**
     * Turn all soft-clipped bases into matches
     */
    RevertSoftclippedBases,
}