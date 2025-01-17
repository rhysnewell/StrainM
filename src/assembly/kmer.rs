use std::cmp::min;
use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};

/**
 * Fast wrapper for byte[] kmers
 *
 * This objects has several important features that make it better than using a raw byte[] for a kmer:
 *
 * -- Can create kmer from a range of a larger byte[], allowing us to avoid Array.copyOfRange
 * -- Fast equals and hashcode methods
 * -- can get actual byte[] of the kmer, even if it's from a larger byte[], and this operation
 *    only does the work of that operation once, updating its internal state
 */
#[derive(Debug, Clone)]
pub struct Kmer {
    // this values may be updated in the course of interacting with this kmer
    // pub bases: &'a [u8],
    start: usize,
    // two constants
    length: usize,
    hash: usize,
}

// TODO: Change Kmer to take a reference to a sequence and have a lifetime
impl Kmer {
    /**
     * Create a new kmer using all bases in kmer
     * @param kmer a non-null byte[]. The input array must not be modified by the caller.
     */
    pub fn new(kmer: &[u8]) -> Self {
        let hash = Self::hash_code(kmer, 0, kmer.len());
        Self {
            start: 0,
            length: kmer.len(),
            hash,
        }
    }

    /**
     * Create a new kmer backed by the bases in bases, spanning start -> start + length
     *
     * Under no circumstances can bases be modified anywhere in the client code.  This does not make a copy
     * of bases for performance reasons
     *
     * @param bases an array of bases
     * @param start the start of the kmer in bases, must be >= 0 and < bases.length
     * @param length the length of the kmer.  Must be >= 0 and start + length < bases.length
     */
    pub fn new_with_start_and_length(bases: &[u8], start: usize, length: usize) -> Self {
        let hash = Self::hash_code(bases, start, length);

        Self {
            start,
            length,
            hash,
        }
    }

    /**
     * Create a derived shallow kmer that starts at newStart and has newLength bases
     * @param newStart the new start of kmer, where 0 means that start of the kmer, 1 means skip the first base
     * @param newLength the new length
     * @return a new kmer based on the data in this kmer.  Does not make a copy, so shares most of the data
     */
    pub fn sub_kmer(&self, bases: &[u8], new_start: usize, new_length: usize) -> Self {
        Self::new_with_start_and_length(bases, self.start + new_start, new_length)
    }

    ///
    /// Compute the hashcode for a KMer.
    ///
    fn hash_code(bases: &[u8], start: usize, length: usize) -> usize {
        if length == 0 {
            return 0;
        }

        let stop = min(start + length, bases.len());
        
        let mut hasher = DefaultHasher::new();
        bases[start..stop].hash(&mut hasher);
        let hash = hasher.finish() as usize;
        
        // for i in start..stop {
        //     h = 31 * h + bases[i] as usize;
        // }

        hash
    }

    ///
    /// Get the bases of this kmer.
    ///
    /// The bases aren't stored in the kmer object to avoid excess copying/cloning. So the full sequence
    /// must be passed by reference and the kmer is retrieved as a slice
    ///
    /// returns a byte slice of the bases of this kmer
    ///
    pub fn bases<'a>(&self, sequence: &'a [u8]) -> &'a [u8] {
        &sequence[self.start..min(self.start + self.length, sequence.len())]
    }

    pub fn len(&self) -> usize {
        self.length
    }

    // /**
    //  * Gets a set of differing positions and bases from another k-mer, limiting up to a max distance.
    //  * For example, if this = "ACATT" and other = "ACGGT":
    //  * - if maxDistance < 2 then -1 will be returned, since distance between kmers is 2.
    //  * - If maxDistance >=2, then 2 will be returned, and arrays will be filled as follows:
    //  * differingIndeces = {2,3}
    //  * differingBases = {'G','G'}
    //  * @param other                 Other k-mer to test
    //  * @param maxDistance           Maximum distance to search. If this and other k-mers are beyond this Hamming distance,
    //  *                              search is aborted and -1 is returned
    //  * @param differingIndeces      Array with indices of differing bytes in array
    //  * @param differingBases        Actual differing bases
    //  * @return                      Set of mappings of form (int->byte), where each elements represents index
    //  *                              of k-mer array where bases mismatch, and the byte is the base from other kmer.
    //  *                              If both k-mers differ by more than maxDistance, returns null
    //  */
    // pub fn get_differing_positions(
    //     &self,
    //     sequence: &[u8],
    //     other: &Self,
    //     other_sequence: &[u8],
    //     max_distance: usize,
    //     differing_indices: &mut Vec<usize>,
    //     differing_bases: &mut Vec<u8>,
    // ) -> i32 {
    //     let mut dist = 0;
    //     if self.length == other.length {
    //         let f2 = &other.bases;
    //         for i in 0..self.length {
    //             if self.bases[self.start + i] != f2[i] {
    //                 differing_indices[dist] = i;
    //                 differing_bases[dist + 1] = f2[i];
    //                 dist += 1;
    //                 if dist > max_distance {
    //                     return -1;
    //                 }
    //             }
    //         }
    //     }
    //     return dist as i32;
    // }

    pub fn to_string(&self) -> String {
        return format!("Kmer{{{}}}", format!("{}{}", self.start, self.length));
    }
}

impl PartialEq for Kmer {
    fn eq(&self, other: &Self) -> bool {
        self.hash == other.hash && self.length == other.length
    }
}

impl Eq for Kmer {}

impl Hash for Kmer {
    fn hash<H: Hasher>(&self, state: &mut H) {
        self.hash.hash(state)
    }
}
