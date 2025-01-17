// #![allow(
//     non_upper_case_globals,
//     non_snake_case
// )]


// use lorikeet_genome::reads::bird_tool_reads::BirdToolRead;
// use lorikeet_genome::utils::artificial_read_utils::ArtificialReadUtils;

// static DEBUG: bool = true;
// static REF_CHUNK: &'static str = "GCATAAACATGGCTCACTGC";
// static REF_CHUNK_HARD: &'static str = "AGCCTTGAACTCCTGGGCTCAAGTGATCCTCCTGCCTCAGTTTCCCATGTAGCTGGGACCACAGGTGGGGGCTCCACCCCTGGCTGATTTTTTTTTTTTTTTTTTTTTGAGATAGGGT";
// //TODO: These tests seem rather bare bones, Revisit and get more rigorous testing done

// #[test]
// fn test_basic_correction_set() {
//     let true_bases = REF_CHUNK.as_bytes();
//     let num_corrections = 50;
//     let mut correction_set = CorrectionSet::new(true_bases.len());

//     let mut offset = 2;
//     for _ in 0..num_corrections {
//         // introduce one correction at a random offset in array. To make testing easier, we will replicate corrrection
//         let base = true_bases[offset];
//         correction_set.add(offset, base);
//         offset += 7;
//         if offset >= true_bases.len() {
//             offset -= true_bases.len();
//         };
//     }

//     for k in 0..true_bases.len() {
//         let corr = correction_set.get_consensus_correction(k);
//         assert_eq!(corr.unwrap(), true_bases[k]);
//     }
// }

// #[test]
// fn test_extended_correction_set() {
//     let true_bases = REF_CHUNK.as_bytes();
//     let num_corrections = 50;
//     let mut correction_set = CorrectionSet::new(true_bases.len());

//     for offset in 0..true_bases.len() {
//         // insert k corrections at offset k and make sure we get exactly k bases back
//         for _ in 0..offset {
//             correction_set.add(offset, true_bases[offset]);
//         }
//     }

//     for offset in 0..true_bases.len() {
//         assert_eq!(correction_set.corrections[offset].len(), offset)
//     }
// }

// #[test]
// fn test_add_reads_to_kmers() {
//     let NUM_GOOD_READS = 500;
//     let bases = "AAAAAAAAAAAAAAA";
//     let READ_LENGTH = bases.len();
//     let kmer_length_for_read_error_correction = READ_LENGTH;

//     let mut read_list = Vec::with_capacity(NUM_GOOD_READS);
//     let mut offset = 0;
//     let quals = vec![30; READ_LENGTH];

//     for k in 0..NUM_GOOD_READS {
//         let read = ArtificialReadUtils::create_artificial_read(
//             bases.as_bytes(),
//             &quals,
//             CigarString(vec![Cigar::Match(READ_LENGTH as u32)]),
//         );
//         read_list.push(read);
//     }

//     let mut read_error_corrector = NearbyKmerErrorCorrector::default(
//         kmer_length_for_read_error_correction,
//         6,
//         10,
//         REF_CHUNK_HARD.as_bytes(),
//     );
//     let finalized_read_list = read_list.iter().collect::<Vec<&BirdToolRead>>();
//     read_error_corrector.add_reads_to_kmers(finalized_read_list);

//     // special trivial case: kmer length is equal to read length.
//     // K-mer counter should hold then exactly one kmer
//     assert_eq!(
//         read_error_corrector
//             .counts_by_kmer
//             .get_counted_kmers()
//             .len(),
//         1
//     );
//     for kmer in read_error_corrector.counts_by_kmer.get_counted_kmers() {
//         assert_eq!(kmer.get_kmer().bases.as_slice(), bases.as_bytes());
//         assert_eq!(kmer.get_count(), NUM_GOOD_READS);
//     }

//     // special case 2: kmers are all the same but length < read length.
//     // Each kmer is added then readLength-kmerLength+1 times
//     let KMER_LENGTH = 10;
//     let mut read_error_corrector =
//         NearbyKmerErrorCorrector::default(KMER_LENGTH, 6, 10, REF_CHUNK_HARD.as_bytes());
//     let finalized_read_list = read_list.iter().collect::<Vec<&BirdToolRead>>();
//     read_error_corrector.add_reads_to_kmers(finalized_read_list);
//     assert_eq!(
//         read_error_corrector
//             .counts_by_kmer
//             .get_counted_kmers()
//             .len(),
//         1
//     );
//     for kmer in read_error_corrector.counts_by_kmer.get_counted_kmers() {
//         assert_eq!(
//             kmer.get_count(),
//             NUM_GOOD_READS * (READ_LENGTH - KMER_LENGTH + 1)
//         )
//     }
// }

// #[test]
// fn test_basic_error_correction() {
//     let NUM_GOOD_READS = 500;
//     let NUM_BAD_READS = 10;
//     let READ_LENGTH = 15;
//     let kmer_length_for_read_error_correction = 10;
//     let mut read_list = Vec::new();
//     let mut offset = 0;
//     let quals = vec![30; READ_LENGTH];

//     for k in 0..NUM_GOOD_READS {
//         let bases = &REF_CHUNK.as_bytes()[offset..(offset + READ_LENGTH)];
//         let read = ArtificialReadUtils::create_artificial_read(
//             bases,
//             &quals,
//             CigarString(vec![Cigar::Match(READ_LENGTH as u32)]),
//         );
//         read_list.push(read);
//         offset += 1;
//         if offset >= REF_CHUNK.len() - READ_LENGTH {
//             offset = 0
//         };
//     }

//     offset = 2;

//     // coverage profile is now perfectly triangular with "good" bases. Inject now bad bases with errors in them.
//     for k in 0..NUM_BAD_READS {
//         let mut bases = read_list[k].read.seq().as_bytes();
//         bases[offset] = b'N';
//         let read = ArtificialReadUtils::create_artificial_read(
//             bases.as_slice(),
//             &quals,
//             CigarString(vec![Cigar::Match(READ_LENGTH as u32)]),
//         );
//         read_list.push(read);
//         offset += 7;
//         if offset >= READ_LENGTH {
//             offset = 4; // just some randomly circulating offset for error position
//         }
//     }

//     // now correct all reads
//     let mut read_error_corrector = NearbyKmerErrorCorrector::default(
//         kmer_length_for_read_error_correction,
//         6,
//         10,
//         REF_CHUNK_HARD.as_bytes(),
//     );
//     let corrected_reads = read_error_corrector.correct_reads(read_list.clone());

//     // check that corrected reads have exactly same content as original reads
//     // I thought this would be correcting the bad reads, but apparently not?
//     for k in 0..NUM_BAD_READS {
//         let bad_bases = read_list[NUM_GOOD_READS + k].read.seq().as_bytes();
//         let corrected = corrected_reads[NUM_GOOD_READS + k].read.seq().as_bytes();

//         assert_eq!(
//             bad_bases,
//             corrected,
//             "original {} corrected {}",
//             std::str::from_utf8(bad_bases.as_slice()).unwrap(),
//             std::str::from_utf8(corrected.as_slice()).unwrap()
//         );
//     }
// }
