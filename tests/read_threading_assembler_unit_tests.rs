#![allow(
    non_upper_case_globals,
    non_snake_case
)]

#[macro_use]
extern crate lazy_static;

use bio::io::fasta::IndexedReader;
use gkl::smithwaterman::Parameters;
use lorikeet_genome::assembly::assembly_region::AssemblyRegion;
use lorikeet_genome::assembly::assembly_result_set::AssemblyResultSet;
use lorikeet_genome::graphs::base_edge::BaseEdgeStruct;
use lorikeet_genome::graphs::graph_based_k_best_haplotype_finder::GraphBasedKBestHaplotypeFinder;
use lorikeet_genome::graphs::seq_graph::SeqGraph;
use lorikeet_genome::haplotype::haplotype::Haplotype;
use lorikeet_genome::model::byte_array_allele::{Allele, ByteArrayAllele};
use lorikeet_genome::model::variant_context::VariantContext;
use lorikeet_genome::pair_hmm::pair_hmm_likelihood_calculation_engine::AVXMode;
use lorikeet_genome::read_threading::read_threading_assembler::ReadThreadingAssembler;
use lorikeet_genome::read_threading::read_threading_graph::ReadThreadingGraph;
use lorikeet_genome::reads::bird_tool_reads::BirdToolRead;
use lorikeet_genome::reference::reference_reader_utils::ReferenceReaderUtils;
use lorikeet_genome::smith_waterman::smith_waterman_aligner::{
    NEW_SW_PARAMETERS, STANDARD_NGS,
};
use lorikeet_genome::utils::artificial_read_utils::ArtificialReadUtils;
use lorikeet_genome::utils::simple_interval::{Locatable, SimpleInterval};
use petgraph::stable_graph::NodeIndex;
use rust_htslib::bam::record::{Cigar, CigarString};
use std::collections::HashSet;
use std::fs::File;

lazy_static! {
    static ref DANGLING_END_SW_PARAMETERS: Parameters = *STANDARD_NGS;
    static ref HAPLOTYPE_TO_REFERENCE_SW_PARAMETERS: Parameters = *NEW_SW_PARAMETERS;
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

// #[test]
fn make_assemble_intervals_data() {
    let start = 100000;
    let end = 200001;
    let window_size = 100;
    let step_size = 200;
    let n_reads_to_use = 5;
    let mut reader = ReferenceReaderUtils::retrieve_reference(&Some(
        "tests/resources/large/Homo_sapiens_assembly19_chr1_1M.fasta".to_string(),
    ));

    for start_i in (start..end).step_by(step_size) {
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

fn assemble(
    assembler: &mut ReadThreadingAssembler,
    ref_bases: &[u8],
    loc: SimpleInterval,
    contig_len: usize,
    reads: Vec<BirdToolRead>,
    ref_haplotype: &mut Haplotype<SimpleInterval>,
) -> AssemblyResultSet<ReadThreadingGraph> {
    let cigar = CigarString(vec![Cigar::Match(ref_haplotype.get_bases().len() as u32)]);
    ref_haplotype.set_cigar(cigar.0);

    let mut active_region =
        AssemblyRegion::new(loc.clone(), true, 0, contig_len, loc.get_contig(), 0, 0.0);
    active_region.add_all(reads);
    let samples = vec!["sample_1".to_string()];
    

    assembler.run_local_assembly(
        active_region,
        ref_haplotype,
        ref_bases.to_vec(),
        loc,
        &samples,
        *STANDARD_NGS,
        *NEW_SW_PARAMETERS,
        AVXMode::detect_mode(),
        None
    )
}

fn test_assemble_ref_and_snp(
    assembler: ReadThreadingAssembler,
    loc: SimpleInterval,
    n_reads_to_use: usize,
    variant_site: usize,
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

    let ref_base = ByteArrayAllele::new(&ref_bases[variant_site..=variant_site], true);
    let alt_base = ByteArrayAllele::new(
        if ref_base.get_bases()[0] == b'A' {
            b"C"
        } else {
            b"A"
        },
        false,
    );

    let vcb = VariantContext::build(
        loc.get_contig(),
        variant_site,
        variant_site,
        vec![ref_base, alt_base],
    );
    test_assembly_with_variant(assembler, &ref_bases, loc, n_reads_to_use, vcb, contig_len);
}

fn test_assemble_ref_and_deletion(
    _assembler: ReadThreadingAssembler,
    loc: SimpleInterval,
    n_reads_to_use: usize,
    variant_site: usize,
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

    for deletion_length in 1..10 {
        let ref_base = ByteArrayAllele::new(
            &ref_bases[variant_site..=(variant_site + deletion_length + 1)],
            true,
        );
        let alt_base = ByteArrayAllele::new(&ref_base.get_bases()[0..=0], false);
        let vcb = VariantContext::build(
            loc.get_contig(),
            variant_site,
            variant_site + deletion_length,
            vec![ref_base, alt_base],
        );
        let assembler = ReadThreadingAssembler::default();

        test_assembly_with_variant(
            assembler,
            &ref_bases,
            loc.clone(),
            n_reads_to_use,
            vcb,
            contig_len,
        );
    }
}

fn test_assemble_ref_and_insertion(
    _assembler: ReadThreadingAssembler,
    loc: SimpleInterval,
    n_reads_to_use: usize,
    variant_site: usize,
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
    println!("Ref bases {}", ref_bases.len());
    for insertion_length in 1..10 {
        let ref_base = ByteArrayAllele::new(&ref_bases[variant_site..=variant_site], true);
        let alt_base = ByteArrayAllele::new(
            &ref_bases[variant_site..=(variant_site + insertion_length + 1)],
            false,
        );
        let assembler = ReadThreadingAssembler::default();

        let vcb = VariantContext::build(
            loc.get_contig(),
            variant_site,
            variant_site + insertion_length,
            vec![ref_base, alt_base],
        );
        test_assembly_with_variant(
            assembler,
            &ref_bases,
            loc.clone(),
            n_reads_to_use,
            vcb,
            contig_len,
        );
    }
}

fn test_assembly_with_variant(
    mut assembler: ReadThreadingAssembler,
    ref_bases: &[u8],
    loc: SimpleInterval,
    n_reads_to_use: usize,
    site: VariantContext,
    contig_len: usize,
) {
    let pre_ref = std::str::from_utf8(&ref_bases[0..site.loc.get_start()]).unwrap();
    let post_ref =
        std::str::from_utf8(&ref_bases[site.loc.get_end() + 1..ref_bases.len()]).unwrap();
    let alt_bases = format!(
        "{}{}{}",
        pre_ref,
        std::str::from_utf8(site.get_alternate_alleles()[0].get_bases()).unwrap(),
        post_ref
    );

    let mut reads = Vec::new();
    let mut counter = 0;
    let quals = vec![30; alt_bases.len()];
    let cigar = format!("{}M", alt_bases.len());
    for _i in 0..n_reads_to_use {
        let bases = alt_bases.as_bytes();
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

    let mut ref_haplotype = Haplotype::new(ref_bases, true);
    let alt_haplotype = Haplotype::new(alt_bases.as_bytes(), false);
    let ref_haplotype_clone = ref_haplotype.clone();
    let haplotypes = assemble(
        &mut assembler,
        ref_bases,
        loc,
        contig_len,
        reads,
        &mut ref_haplotype,
    );

    println!(
        "{}",
        std::str::from_utf8(ref_haplotype_clone.get_bases()).unwrap()
    );
    println!(
        "{}",
        std::str::from_utf8(alt_haplotype.get_bases()).unwrap()
    );

    assert_eq!(
        haplotypes.get_haplotype_list(),
        vec![ref_haplotype_clone, alt_haplotype]
    );
}

// #[test]
fn make_assemble_intervals_with_variant_data() {
    let start = 100000;
    let end = 101001;
    let window_size = 100;
    let step_size = 200;
    let variant_step_size = 1;
    let n_reads_to_use = 5;
    let mut reader = ReferenceReaderUtils::retrieve_reference(&Some(
        "tests/resources/large/Homo_sapiens_assembly19_chr1_1M.fasta".to_string(),
    ));

    for start_i in (start..end).step_by(step_size) {
        let end_i = start_i + window_size;
        let ref_loc = SimpleInterval::new(0, start_i, end_i);
        for variant_start in (((window_size / 2) - 10)..((window_size / 2) + 10))
            .step_by(variant_step_size)
        {
            test_assemble_ref_and_snp(
                ReadThreadingAssembler::default(),
                ref_loc.clone(),
                n_reads_to_use,
                variant_start,
                &mut reader,
            );
        }
    }
}

// #[test]
fn make_assemble_intervals_with_deletion_data() {
    let start = 100000;
    let end = 101001;
    let window_size = 100;
    let step_size = 200;
    let variant_step_size = 1;
    let n_reads_to_use = 5;
    let mut reader = ReferenceReaderUtils::retrieve_reference(&Some(
        "tests/resources/large/Homo_sapiens_assembly19_chr1_1M.fasta".to_string(),
    ));

    for start_i in (start..end).step_by(step_size) {
        let end_i = start_i + window_size;
        let ref_loc = SimpleInterval::new(0, start_i, end_i);
        for variant_start in (((window_size / 2) - 10)..((window_size / 2) + 10))
            .step_by(variant_step_size)
        {
            test_assemble_ref_and_deletion(
                ReadThreadingAssembler::default(),
                ref_loc.clone(),
                n_reads_to_use,
                variant_start,
                &mut reader,
            );
        }
    }
}

// #[test]
fn make_assemble_intervals_with_insertion_data() {
    let start = 100000;
    let end = 101001;
    let window_size = 100;
    let step_size = 200;
    let variant_step_size = 1;
    let n_reads_to_use = 5;
    let mut reader = ReferenceReaderUtils::retrieve_reference(&Some(
        "tests/resources/large/Homo_sapiens_assembly19_chr1_1M.fasta".to_string(),
    ));

    for start_i in (start..end).step_by(step_size) {
        let end_i = start_i + window_size;
        let ref_loc = SimpleInterval::new(0, start_i, end_i);
        for variant_start in (((window_size / 2) - 10)..((window_size / 2) + 10))
            .step_by(variant_step_size)
        {
            test_assemble_ref_and_insertion(
                ReadThreadingAssembler::default(),
                ref_loc.clone(),
                n_reads_to_use,
                variant_start,
                &mut reader,
            );
        }
    }
}

fn test_simple_assembly(
    _name: &str,
    mut assembler: ReadThreadingAssembler,
    loc: SimpleInterval,
    reference: &str,
    alt: &str,
    contig_len: usize,
) {
    let ref_bases = reference.as_bytes();
    let alt_bases = alt.as_bytes();

    let quals = vec![30; alt_bases.len()];
    let cigar = format!("{}M", alt_bases.len());
    let mut reads = Vec::new();
    for _i in 0..20 {
        let bases = alt_bases;
        let quals = quals.as_slice();
        let cigar = cigar.as_str();
        let read = ArtificialReadUtils::create_artificial_read_with_name_and_pos(
            "test".to_string(),
            loc.get_contig() as i32,
            loc.get_start() as i64,
            bases,
            quals,
            cigar,
            0,
        );
        reads.push(read);
    }

    let mut ref_haplotype = Haplotype::new(ref_bases, true);
    let alt_haplotype = Haplotype::new(alt_bases, false);
    let haplotypes = assemble(
        &mut assembler,
        ref_bases,
        loc,
        contig_len,
        reads,
        &mut ref_haplotype,
    )
    .get_haplotype_list();
    assert!(!haplotypes.is_empty(), "Failed to find ref haplotype");
    assert_eq!(&haplotypes[0], &ref_haplotype);

    assert_eq!(haplotypes.len(), 2, "Failed to find single alt haplotype");
    assert_eq!(&haplotypes[1], &alt_haplotype);
}

// #[test]
fn make_simple_assembly_test_data() {
    let start = 100000;
    let window_size = 200;
    let end = start + window_size;

    let exclude_variants_within_x_bp = 25; // TODO -- decrease to zero when the edge calling problem is fixed
    let mut reader = ReferenceReaderUtils::retrieve_reference(&Some(
        "tests/resources/large/Homo_sapiens_assembly19_chr1_1M.fasta".to_string(),
    ));
    reader.fetch_by_rid(0, start, end + 1);
    let contig_len = reader.index.sequences()[0].len as usize;
    let mut ref_bases = Vec::new();
    reader.read(&mut ref_bases);
    let ref_loc = SimpleInterval::new(0, start as usize, end as usize);

    for snp_pos in 0..window_size {
        if snp_pos > exclude_variants_within_x_bp
            && (window_size - snp_pos) >= exclude_variants_within_x_bp
        {
            let mut alt_bases = ref_bases.clone();
            alt_bases[snp_pos as usize] = if alt_bases[snp_pos as usize] == b'A' {
                b'C'
            } else {
                b'A'
            };
            let alt = std::str::from_utf8(&alt_bases).unwrap();
            let name = format!("snp at {}", snp_pos);
            test_simple_assembly(
                name.as_str(),
                ReadThreadingAssembler::default(),
                ref_loc.clone(),
                std::str::from_utf8(&ref_bases).unwrap(),
                alt,
                contig_len,
            );
        }
    }
}

struct TestAssembler {
    assembler: ReadThreadingAssembler,
    ref_haplotype: Haplotype<SimpleInterval>,
    reads: Vec<BirdToolRead>,
}

impl TestAssembler {
    fn new(kmer_size: usize) -> Self {
        let mut assembler = ReadThreadingAssembler::default_with_kmers(100000, vec![kmer_size], 0);
        assembler.set_just_return_raw_graph(true);
        Self {
            assembler,
            ref_haplotype: Haplotype::no_call(),
            reads: Vec::new(),
        }
    }

    fn add_sequence(&mut self, bases: &[u8], is_ref: bool) {
        if is_ref {
            self.ref_haplotype = Haplotype::new(bases, true);
        } else {
            let quals = vec![30; bases.len()];
            let read = ArtificialReadUtils::create_artificial_read(
                bases,
                quals.as_slice(),
                CigarString::from(vec![Cigar::Match(bases.len() as u32)]),
            );
            self.reads.push(read);
        }
    }

    fn assemble(&mut self) -> SeqGraph<BaseEdgeStruct> {
        self.assembler.set_remove_paths_not_connected_to_ref(false);
        self.assembler.set_recover_dangling_branches(false);
        let reads = self.reads.clone();
        let ref_haplotype = self.ref_haplotype.clone();
        let sample_names = vec!["SampleX".to_string()];
        let graph = self.assembler.assemble(
            &reads,
            &ref_haplotype,
            sample_names.as_slice(),
            &DANGLING_END_SW_PARAMETERS,
            AVXMode::detect_mode(),
            None
        )[0]
        .clone()
        .get_seq_graph();

        graph.unwrap()
    }
}

fn assert_linear_graph(assembler: &mut TestAssembler, seq: String) {
    let mut graph = assembler.assemble();
    graph.simplify_graph("");
    assert_eq!(graph.base_graph.vertex_set().len(), 1);
    assert_eq!(
        graph
            .base_graph
            .vertex_set()
            .into_iter()
            .map(|v| String::from_utf8(v.sequence.clone()).unwrap())
            .collect::<Vec<String>>(),
        vec![seq]
    );
}

fn assert_single_bubble(assembler: &mut TestAssembler, one: String, two: String) {
    let mut graph = assembler.assemble();
    graph.simplify_graph("");

    let sources = graph
        .base_graph
        .get_sources_generic()
        .collect::<HashSet<NodeIndex>>();
    let sinks = graph
        .base_graph
        .get_sinks_generic()
        .collect::<HashSet<NodeIndex>>();
    let paths = GraphBasedKBestHaplotypeFinder::new(&mut graph.base_graph, sources, sinks)
        .find_best_haplotypes(usize::MAX, &graph.base_graph);
    assert_eq!(paths.len(), 2);

    let mut expected = HashSet::new();
    expected.insert(one);
    expected.insert(two);

    for path in paths {
        let seq = String::from_utf8(path.path.get_bases(&graph.base_graph)).unwrap();
        assert!(expected.contains(&seq));
        expected.remove(&seq);
    }
}

#[test]
fn test_ref_creation() {
    let reference = "ACGTAACCGGTT";
    let mut assembler = TestAssembler::new(3);
    assembler.add_sequence(reference.as_bytes(), true);
    assert_linear_graph(&mut assembler, reference.to_string());
}

#[test]
fn test_ref_non_unique_creation() {
    let reference = "GAAAAT";
    let mut assembler = TestAssembler::new(3);
    assembler.add_sequence(reference.as_bytes(), true);
    assert_linear_graph(&mut assembler, reference.to_string());
}

#[test]
fn test_ref_alt_creation() {
    let reference = "ACAACTGA";
    let alternate = "ACAGCTGA";
    let mut assembler = TestAssembler::new(3);

    assembler.add_sequence(reference.as_bytes(), true);
    assembler.add_sequence(alternate.as_bytes(), false);

    assert_single_bubble(&mut assembler, reference.to_string(), alternate.to_string());
}

#[test]
fn test_partial_reads_creation() {
    let reference = "ACAACTGA";
    let alternate1 = "ACAGCT";
    let alternate2 = "GCTGA";
    let mut assembler = TestAssembler::new(3);

    assembler.add_sequence(reference.as_bytes(), true);
    assembler.add_sequence(alternate1.as_bytes(), false);
    assembler.add_sequence(alternate2.as_bytes(), false);

    assert_single_bubble(
        &mut assembler,
        reference.to_string(),
        "ACAGCTGA".to_string(),
    );
}

#[test]
fn test_mismatch_in_first_kmer() {
    let reference = "ACAACTGA";
    let alternate = "AGCTGA";
    let mut assembler = TestAssembler::new(3);

    assembler.add_sequence(reference.as_bytes(), true);
    assembler.add_sequence(alternate.as_bytes(), false);

    let mut graph = assembler.assemble();
    graph.simplify_graph("");
    graph.base_graph.remove_singleton_orphan_vertices();

    let sources = graph
        .base_graph
        .get_sources_generic()
        .collect::<HashSet<NodeIndex>>();
    let sinks = graph
        .base_graph
        .get_sinks_generic()
        .collect::<HashSet<NodeIndex>>();

    assert_eq!(sources.len(), 1);
    assert_eq!(sinks.len(), 1);

    assert!(graph.base_graph.get_reference_sink_vertex().is_some());
    assert!(graph.base_graph.get_reference_source_vertex().is_some());
    let paths = GraphBasedKBestHaplotypeFinder::new(&mut graph.base_graph, sources, sinks)
        .find_best_haplotypes(usize::MAX, &graph.base_graph);

    assert_eq!(paths.len(), 1);
}

#[test]
fn test_starts_in_middle() {
    let reference = "CAAAATG";
    let alternate = "AAATG";
    let mut assembler = TestAssembler::new(3);

    assembler.add_sequence(reference.as_bytes(), true);
    assembler.add_sequence(alternate.as_bytes(), false);

    assert_linear_graph(&mut assembler, reference.to_string());
}

#[test]
fn test_starts_in_middle_with_single_bubble() {
    let reference = "CAAAATGGGG";
    let alternate = "AAATCGGG";
    let mut assembler = TestAssembler::new(3);

    assembler.add_sequence(reference.as_bytes(), true);
    assembler.add_sequence(alternate.as_bytes(), false);

    assert_single_bubble(
        &mut assembler,
        reference.to_string(),
        "CAAAATCGGG".to_string(),
    );
}

#[test]
fn test_single_indel_as_double_indel_3_reads() {
    let reference  = "GTTTTTCCTAGGCAAATGGTTTCTATAAAATTATGTGTGTGTGTCTCTCTCTGTGTGTGTGTGTGTGTGTGTGTGTATACCTAATCTCACACTCTTTTTTCTGG";
    let alternate1 = "GTTTTTCCTAGGCAAATGGTTTCTATAAAATTATGTGTGTGTGTCTCTGTGTGTGTGTGTGTGTGTATACCTAATCTCACACTCTTTTTTCTGG";
    let alternate2 = "GTTTTTCCTAGGCAAATGGTTTCTATAAAATTATGTGTGTGTGTCTCTGTGTGTGTGTGTGTGTGTATACCTAATCTCACACTCTTTTTTCTGG";

    let mut assembler = TestAssembler::new(25);

    assembler.add_sequence(reference.as_bytes(), true);
    assembler.add_sequence(alternate1.as_bytes(), false);
    assembler.add_sequence(alternate2.as_bytes(), false);

    let mut graph = assembler.assemble();

    let sources = graph
        .base_graph
        .get_sources_generic()
        .collect::<HashSet<NodeIndex>>();
    let sinks = graph
        .base_graph
        .get_sinks_generic()
        .collect::<HashSet<NodeIndex>>();
    let paths = GraphBasedKBestHaplotypeFinder::new(&mut graph.base_graph, sources, sinks)
        .find_best_haplotypes(usize::MAX, &graph.base_graph);

    assert_eq!(paths.len(), 2);
}
