#![allow(
    non_upper_case_globals,
    unused_parens,
    unused_mut,
    unused_imports,
    non_snake_case
)]

extern crate lorikeet_genome;
extern crate rayon;
extern crate rust_htslib;
#[macro_use]
extern crate lazy_static;
#[macro_use]
extern crate approx;
extern crate bio;
extern crate itertools;
extern crate rand;
extern crate term;

use bio::io::fasta::IndexedReader;
use itertools::Itertools;
use lorikeet_genome::assembly::assembly_region::AssemblyRegion;
use lorikeet_genome::assembly::assembly_result_set::AssemblyResultSet;
use lorikeet_genome::genotype::genotype_builder::Genotype;
use lorikeet_genome::genotype::genotype_likelihood_calculators::GenotypeLikelihoodCalculators;
use lorikeet_genome::genotype::genotype_likelihoods::GenotypeLikelihoods;
use lorikeet_genome::haplotype::haplotype::Haplotype;
use lorikeet_genome::model::allele_frequency_calculator::AlleleFrequencyCalculator;
use lorikeet_genome::model::allele_likelihoods::AlleleLikelihoods;
use lorikeet_genome::model::byte_array_allele::{Allele, ByteArrayAllele};
use lorikeet_genome::model::variant_context::VariantContext;
use lorikeet_genome::model::{allele_list::AlleleList, variants::SPAN_DEL_ALLELE};
use lorikeet_genome::pair_hmm::pair_hmm::PairHMM;
use lorikeet_genome::pair_hmm::pair_hmm_likelihood_calculation_engine::PairHMMInputScoreImputator;
use lorikeet_genome::read_error_corrector::nearby_kmer_error_corrector::{
    CorrectionSet, NearbyKmerErrorCorrector,
};
use lorikeet_genome::read_error_corrector::read_error_corrector::ReadErrorCorrector;
use lorikeet_genome::read_threading::read_threading_assembler::ReadThreadingAssembler;
use lorikeet_genome::read_threading::read_threading_graph::ReadThreadingGraph;
use lorikeet_genome::reads::bird_tool_reads::BirdToolRead;
use lorikeet_genome::reads::cigar_utils::CigarUtils;
use lorikeet_genome::reference::reference_reader_utils::ReferenceReaderUtils;
use lorikeet_genome::smith_waterman::bindings::SWParameters;
use lorikeet_genome::smith_waterman::smith_waterman_aligner::{
    ALIGNMENT_TO_BEST_HAPLOTYPE_SW_PARAMETERS, NEW_SW_PARAMETERS, ORIGINAL_DEFAULT, STANDARD_NGS,
};
use lorikeet_genome::test_utils::read_likelihoods_unit_tester::ReadLikelihoodsUnitTester;
use lorikeet_genome::utils::artificial_read_utils::ArtificialReadUtils;
use lorikeet_genome::utils::base_utils::BaseUtils;
use lorikeet_genome::utils::math_utils::{MathUtils, LOG10_ONE_HALF};
use lorikeet_genome::utils::quality_utils::QualityUtils;
use lorikeet_genome::utils::simple_interval::{Locatable, SimpleInterval};
use lorikeet_genome::GenomeExclusionTypes::GenomesAndContigsType;
use rand::rngs::ThreadRng;
use rand::seq::index::sample;
use rayon::prelude::*;
use rust_htslib::bam::ext::BamRecordExtensions;
use rust_htslib::bam::record::{Cigar, CigarString, CigarStringView, Seq};
use std::cmp::{max, min, Ordering};
use std::collections::{HashMap, HashSet};
use std::convert::TryFrom;
use std::fs::File;
use std::ops::Deref;
use std::sync::Mutex;

lazy_static! {
    static ref DANGLING_END_SW_PARAMETERS: SWParameters = *STANDARD_NGS;
    static ref HAPLOTYPE_TO_REFERENCE_SW_PARAMETERS: SWParameters = *NEW_SW_PARAMETERS;
}

// #[test]
// fn test_read_threading_assembler_doesnt_modify_input_kmer_list() {
//     let kmers_out_of_order = vec![65, 25, 45, 35, 85];
//
// }

fn test_assemble_ref(
    mut assembler: ReadThreadingAssembler,
    loc: SimpleInterval,
    n_reads_to_use: usize,
    seq: &mut IndexedReader<File>,
) {
    seq.fetch_by_rid(
        loc.get_contig(),
        loc.get_start() as u64,
        loc.get_end() as u64 + 1,
    );
    let contig_len = seq.index.sequences()[0].len as usize;
    let mut ref_bases = Vec::new();
    seq.read(&mut ref_bases);

    let mut reads = Vec::new();
    let mut counter = 0;
    let cigar = format!("{}M", ref_bases.len());

    for _ in 0..n_reads_to_use {
        let bases = ref_bases.as_slice();
        let quals = vec![30; ref_bases.len()];
        let read = ArtificialReadUtils::create_artificial_read_with_name_and_pos(
            format!("{}_{}", loc.get_contig(), counter),
            loc.tid(),
            loc.get_start() as i64,
            bases,
            quals.as_slice(),
            cigar.as_str(),
            0,
        );
        reads.push(read);
        counter += 1;
    }

    let ref_haplotype_orig = Haplotype::new(ref_bases.as_slice(), true);
    let mut ref_haplotype = Haplotype::new(ref_bases.as_slice(), true);

    let assembly_result_set = assemble(
        &mut assembler,
        ref_bases.as_slice(),
        loc,
        contig_len,
        reads,
        &mut ref_haplotype,
    );
    let haplotypes = assembly_result_set.get_haplotype_list();

    assert_eq!(haplotypes, vec![ref_haplotype_orig]);
}

#[test]
fn make_assemble_intervals_data() {
    let start = 100000;
    let end = 200001;
    let window_size = 100;
    let step_size = 200;
    let n_reads_to_use = 5;
    let mut reader = ReferenceReaderUtils::retrieve_reference(&Some(
        "tests/resources/large/Homo_sapiens_assembly19_chr1_1M.fasta".to_string(),
    ));

    for start_i in (start..end).into_iter().step_by(step_size) {
        let end_i = start_i + window_size;
        let ref_loc = SimpleInterval::new(0, start_i, end_i);
        test_assemble_ref(
            ReadThreadingAssembler::default(),
            ref_loc,
            n_reads_to_use,
            &mut reader,
        );
    }
}

fn assemble<'a>(
    assembler: &'a mut ReadThreadingAssembler,
    ref_bases: &[u8],
    loc: SimpleInterval,
    contig_len: usize,
    reads: Vec<BirdToolRead>,
    ref_haplotype: &'a mut Haplotype<'a, SimpleInterval>,
) -> AssemblyResultSet<'a, ReadThreadingGraph> {
    let cigar = CigarString(vec![Cigar::Match(ref_haplotype.get_bases().len() as u32)]);
    ref_haplotype.set_cigar(cigar.0);

    let mut active_region =
        AssemblyRegion::new(loc.clone(), true, 0, contig_len, loc.get_contig(), 0);
    active_region.add_all(reads);
    let samples = vec!["sample_1".to_string()];
    let assembly_result_set = assembler.run_local_assembly(
        active_region,
        ref_haplotype,
        ref_bases.to_vec(),
        loc.clone(),
        None,
        &samples,
        *STANDARD_NGS,
        *NEW_SW_PARAMETERS,
    );

    return assembly_result_set;
}
