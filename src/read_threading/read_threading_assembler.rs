use gkl::smithwaterman::{OverhangStrategy, Parameters};
use hashlink::LinkedHashMap;
use rayon::prelude::*;
use rust_htslib::bam::record::{Cigar, CigarString};

// use crate::read_error_corrector::nearby_kmer_error_corrector::NearbyKmerErrorCorrector;
use crate::assembly::assembly_region::AssemblyRegion;
use crate::assembly::assembly_result::{AssemblyResult, Status};
use crate::assembly::assembly_result_set::AssemblyResultSet;
use crate::graphs::adaptive_chain_pruner::AdaptiveChainPruner;
use crate::graphs::base_edge::{BaseEdge, BaseEdgeStruct};
use crate::graphs::base_graph::BaseGraph;
use crate::graphs::base_vertex::BaseVertex;
use crate::graphs::chain_pruner::ChainPruner;
use crate::graphs::graph_based_k_best_haplotype_finder::GraphBasedKBestHaplotypeFinder;
use crate::graphs::k_best_haplotype::KBestHaplotype;
use crate::graphs::seq_graph::SeqGraph;
use crate::graphs::seq_vertex::SeqVertex;
use crate::haplotype::haplotype::Haplotype;
use crate::model::byte_array_allele::Allele;
use crate::pair_hmm::pair_hmm_likelihood_calculation_engine::AVXMode;
use crate::graphs::low_weight_chain_pruner::LowWeightChainPruner;
use crate::read_threading::abstract_read_threading_graph::{AbstractReadThreadingGraph, SequenceForKmers};
use crate::read_threading::read_threading_graph::ReadThreadingGraph;
use crate::reads::bird_tool_reads::BirdToolRead;
use crate::reads::cigar_utils::CigarUtils;
use crate::reads::read_clipper::ReadClipper;
use crate::utils::simple_interval::{Locatable, SimpleInterval};

const PRUNE_FACTOR_COVERAGE_THRESHOLD: f64 = 10.0;

#[derive(Debug, Clone)]
pub struct ReadThreadingAssembler {
    pub(crate) kmer_sizes: Vec<usize>,
    dont_increase_kmer_sizes_for_cycles: bool,
    allow_non_unique_kmers_in_ref: bool,
    generate_seq_graph: bool,
    // recover_haplotypes_from_edges_not_covered_in_junction_trees: bool,
    num_pruning_samples: i32,
    disable_prune_factor_correction: bool, // if the region has many reads, having a low prune factor can cause excessive runtimes
    num_best_haplotypes_per_graph: i32,
    prune_before_cycle_counting: bool,
    remove_paths_not_connected_to_ref: bool,
    just_return_raw_graph: bool,
    pub(crate) recover_dangling_branches: bool,
    pub(crate) recover_all_dangling_branches: bool,
    pub(crate) min_dangling_branch_length: i32,
    pub(crate) min_base_quality_to_use_in_assembly: u8,
    prune_factor: usize,
    min_matching_bases_to_dangling_end_recovery: i32,
    chain_pruner: ChainPruner,
    pub(crate) debug_graph_transformations: bool,
    pub(crate) debug_graph_output_path: Option<String>,
    // graph_haplotype_histogram_path: Option<String>,
    pub(crate) graph_output_path: Option<String>,
}

impl ReadThreadingAssembler {
    const DEFAULT_NUM_PATHS_PER_GRAPH: usize = 128;
    const KMER_SIZE_ITERATION_INCREASE: usize = 13;
    const MAX_KMER_ITERATIONS_TO_ATTEMPT: usize = 6;

    /**
     * If false, we will only write out a region around the reference source
     */
    // const PRINT_FILL_GRAPH_FOR_DEBUGGING: bool = true;
    const DEFAULT_MIN_BASE_QUALITY_TO_USE: u8 = 10;
    const MIN_HAPLOTYPE_REFERENCE_LENGTH: u32 = 30;

    pub fn new(
        max_allowed_paths_for_read_threading_assembler: i32,
        mut kmer_sizes: Vec<usize>,
        dont_increase_kmer_sizes_for_cycles: bool,
        allow_non_unique_kmers_in_ref: bool,
        num_pruning_samples: i32,
        prune_factor: usize,
        use_adaptive_pruning: bool,
        initial_error_rate_for_pruning: f64,
        pruning_log_odds_threshold: f64,
        pruning_seeding_log_odds_threshold: f64,
        max_unpruned_variants: usize,
        _use_linked_debruijn_graphs: bool,
        enable_legacy_graph_cycle_detection: bool,
        min_matching_bases_to_dangle_end_recovery: i32,
        disable_prune_factor_correction: bool,
    ) -> ReadThreadingAssembler {
        assert!(
            max_allowed_paths_for_read_threading_assembler >= 1,
            "num_best_haplotypes_per_graph should be >= 1 but got {}",
            max_allowed_paths_for_read_threading_assembler
        );
        kmer_sizes.sort_unstable();

        let chain_pruner = if use_adaptive_pruning {
            ChainPruner::AdaptiveChainPruner(AdaptiveChainPruner::new(
                initial_error_rate_for_pruning,
                pruning_log_odds_threshold,
                pruning_seeding_log_odds_threshold,
                max_unpruned_variants,
            ))
        } else {
            ChainPruner::LowWeightChainPruner(LowWeightChainPruner::new(prune_factor))
        };

        // TODO: //!use_linked_debruijn_graphs should be used for generate_seq_graph
        //      but have not yet implement junction tree method
        ReadThreadingAssembler {
            kmer_sizes,
            dont_increase_kmer_sizes_for_cycles,
            allow_non_unique_kmers_in_ref,
            num_pruning_samples,
            prune_factor,
            chain_pruner,
            generate_seq_graph: true,
            prune_before_cycle_counting: !enable_legacy_graph_cycle_detection,
            remove_paths_not_connected_to_ref: true,
            just_return_raw_graph: false,
            recover_dangling_branches: true,
            recover_all_dangling_branches: false,
            min_dangling_branch_length: 0,
            num_best_haplotypes_per_graph: max_allowed_paths_for_read_threading_assembler,
            min_matching_bases_to_dangling_end_recovery: min_matching_bases_to_dangle_end_recovery,
            // recover_haplotypes_from_edges_not_covered_in_junction_trees: true,
            min_base_quality_to_use_in_assembly: Self::DEFAULT_MIN_BASE_QUALITY_TO_USE,
            debug_graph_transformations: false,
            debug_graph_output_path: Some(format!("graph_debugging")),
            // graph_haplotype_histogram_path: None,
            graph_output_path: None,
            disable_prune_factor_correction
        }
    }

    pub fn default() -> Self {
        Self::new(
            Self::DEFAULT_NUM_PATHS_PER_GRAPH as i32,
            vec![25],
            true,
            true,
            1,
            2,
            false,
            0.001,
            2.0,
            2.0,
            std::usize::MAX,
            false,
            false,
            3,
            false,
        )
    }

    pub fn default_with_kmers(
        max_allowed_paths_for_read_threading_assembler: i32,
        kmer_sizes: Vec<usize>,
        prune_factor: usize,
    ) -> Self {
        Self::new(
            max_allowed_paths_for_read_threading_assembler,
            kmer_sizes,
            true,
            true,
            1,
            prune_factor,
            false,
            0.001,
            2.0,
            2.0,
            std::usize::MAX,
            false,
            false,
            3,
            false
        )
    }

    pub fn set_just_return_raw_graph(&mut self, value: bool) {
        self.just_return_raw_graph = value;
    }

    pub fn set_remove_paths_not_connected_to_ref(&mut self, value: bool) {
        self.remove_paths_not_connected_to_ref = value;
    }

    pub fn set_recover_dangling_branches(&mut self, value: bool) {
        self.recover_dangling_branches = value;
    }

    fn set_prune_factor(&mut self, value: usize) {
        self.prune_factor = value;
        self.chain_pruner.set_prune_factor(value);
    }
    /**
     * Main entry point into the assembly engine. Build a set of deBruijn graphs out of the provided reference sequence and list of reads
     * @param assemblyRegion              AssemblyRegion object holding the reads which are to be used during assembly
     * @param refHaplotype              reference haplotype object
     * @param fullReferenceWithPadding  byte array holding the reference sequence with padding
     * @param refLoc                    GenomeLoc object corresponding to the reference sequence with padding
     * @param readErrorCorrector        a ReadErrorCorrector object, if read are to be corrected before assembly. Can be null if no error corrector is to be used.
     * @param aligner                   {@link SmithWatermanAligner} used to align dangling ends in assembly graphs to the reference sequence
     * @return                          the resulting assembly-result-set
     */
    pub fn run_local_assembly<'b>(
        &mut self,
        mut assembly_region: AssemblyRegion,
        ref_haplotype: &'b mut Haplotype<SimpleInterval>,
        full_reference_with_padding: Vec<u8>,
        ref_loc: SimpleInterval,
        // read_error_corrector: Option<C>,
        sample_names: &'b [String],
        dangling_end_sw_parameters: Parameters,
        reference_to_haplotype_sw_parameters: Parameters,
        avx_mode: AVXMode,
        additional_kmer_sizes: Option<Vec<usize>>
    ) -> AssemblyResultSet<ReadThreadingGraph> {
        assert!(
            full_reference_with_padding.len() == ref_loc.size(),
            "Reference bases and reference loc must be the same size. {} -> {}",
            full_reference_with_padding.len(),
            ref_loc.size()
        );

        // Note that error correction does not modify the original reads,
        // which are used for genotyping TODO this might come before error correction /
        // let mut corrected_reads = assembly_region.get_reads_cloned();
        // match read_error_corrector {
        //     // TODO: Is it possible to perform this
        //     //      without cloning? Perhaps get_reads() should just return ownership of reads?
        //     //      Can't move reads out of assembly region as they are required later on during
        //     //      read threading phase. Very annoying
        //     None => assembly_region.get_reads_cloned(),
        //     Some(mut read_error_corrector) => {
        //         read_error_corrector.correct_reads(assembly_region.get_reads_cloned())
        //     }
        // };

        // Revert clipped bases if necessary (since we do not want to assemble them)
        let corrected_reads = assembly_region.move_reads();
        let corrected_reads = corrected_reads
            .into_par_iter()
            .map(|read| ReadClipper::new(read).hard_clip_soft_clipped_bases())
            .collect::<Vec<BirdToolRead>>();

        // calculate coverage estimate. no. reads / region size
        let old_prune_factor = self.prune_factor;
        if !self.disable_prune_factor_correction && !self.chain_pruner.is_adaptive() {
            let coverage = assembly_region.calculate_coverage(&corrected_reads);
            // debug!("Coverage {} read count {} region size {}", coverage, corrected_reads.len(), assembly_region.get_span().size());
            let new_prune_factor = if coverage > PRUNE_FACTOR_COVERAGE_THRESHOLD {
                2
            } else {
                0
            };
            self.set_prune_factor(new_prune_factor);
        }


        // debug!("Corrected reads {}", corrected_reads.len());
        // let non_ref_rt_graphs: Vec<ReadThreadingGraph> = Vec::new();
        // let non_ref_seq_graphs: Vec<SeqGraph<BaseEdgeStruct>> = Vec::new();
        let active_region_extended_location = assembly_region.get_padded_span();
        ref_haplotype.set_genome_location(active_region_extended_location.clone());

        let mut result_set = AssemblyResultSet::new(
            assembly_region,
            full_reference_with_padding,
            ref_loc.clone(),
            ref_haplotype.clone(),
        );

        // either follow the old method for building graphs and then assembling or assemble and haplotype call before expanding kmers
        if self.generate_seq_graph {
            self.assemble_kmer_graphs_and_haplotype_call(
                &ref_haplotype,
                &ref_loc,
                &corrected_reads,
                &mut result_set,
                &active_region_extended_location,
                sample_names,
                &dangling_end_sw_parameters,
                &reference_to_haplotype_sw_parameters,
                avx_mode,
                additional_kmer_sizes
            );
        } else {
            self.assemble_graphs_and_expand_kmers_given_haplotypes(
                &ref_haplotype,
                &ref_loc,
                &corrected_reads,
                &mut result_set,
                &active_region_extended_location,
                sample_names,
                &dangling_end_sw_parameters,
                &reference_to_haplotype_sw_parameters,
                avx_mode,
                additional_kmer_sizes
            )
        }

        // reset prune_factor
        self.set_prune_factor(old_prune_factor);

        // If we get to this point then no graph worked... thats bad and indicates something
        // horrible happened, in this case we just return a reference haplotype
        result_set.region_for_genotyping.reads = corrected_reads;
        // debug!(
        //     "Found {} to compare every read against",
        //     result_set.haplotypes.len()
        // );
        result_set
    }

    /**
     * Follow the old behavior, call into {@link #assemble(List, Haplotype, SAMFileHeader, SmithWatermanAligner)} to decide if a graph
     * is acceptable for haplotype discovery then detect haplotypes.
     */
    fn assemble_kmer_graphs_and_haplotype_call<'b>(
        &mut self,
        ref_haplotype: &'b Haplotype<SimpleInterval>,
        ref_loc: &'b SimpleInterval,
        corrected_reads: &'b Vec<BirdToolRead>,
        // non_ref_seq_graphs: &mut Vec<SeqGraph<BaseEdgeStruct>>,
        result_set: &mut AssemblyResultSet<ReadThreadingGraph>,
        active_region_extended_location: &'b SimpleInterval,
        sample_names: &'b [String],
        dangling_end_sw_parameters: &Parameters,
        reference_to_haplotype_sw_parameters: &Parameters,
        avx_mode: AVXMode,
        additional_kmer_sizes: Option<Vec<usize>>
    ) {
        // create the graphs by calling our subclass assemble method
        self.assemble(
            &corrected_reads,
            ref_haplotype,
            sample_names,
            dangling_end_sw_parameters,
            avx_mode,
            additional_kmer_sizes
        )
        .into_iter()
        .for_each(|mut result| {
            // debug!("graph after assembly {:?}", &result.graph.as_ref().unwrap().base_graph);
            // debug!(
            //     "Result loc {:?} Status {:?} haps {:?}",
            //     active_region_extended_location, &result.status, &result.discovered_haplotypes
            // );

            if result.status == Status::AssembledSomeVariation {
                // do some QC on the graph
                Self::sanity_check_graph(&result.graph.as_ref().unwrap().base_graph, ref_haplotype);
                // add it to graphs with meaningful non-reference features
                self.find_best_path(
                    &mut result,
                    ref_haplotype,
                    ref_loc,
                    active_region_extended_location,
                    reference_to_haplotype_sw_parameters,
                    result_set,
                    avx_mode,
                );
                // non_ref_seq_graphs.push(result.graph.unwrap());
                // result_set.add_haplotype(result);
            }
        });
    }

    /**
     * Given reads and a reference haplotype give us graphs to use for constructing
     * non-reference haplotypes.
     *
     * @param reads the reads we're going to assemble
     * @param refHaplotype the reference haplotype
     * @param aligner {@link SmithWatermanAligner} used to align dangling ends in assembly graphs to the reference sequence
     * @return a non-null list of reads
     */
    pub fn assemble<'b>(
        &mut self,
        reads: &'b Vec<BirdToolRead>,
        ref_haplotype: &'b Haplotype<SimpleInterval>,
        sample_names: &'b [String],
        dangling_end_sw_parameters: &Parameters,
        avx_mode: AVXMode,
        additional_kmer_sizes: Option<Vec<usize>>,
    ) -> Vec<AssemblyResult<SimpleInterval, ReadThreadingGraph>> {

        let mut kmer_sizes = self.kmer_sizes.clone();
        if let Some(additional) = additional_kmer_sizes {
            kmer_sizes.extend(additional);
        }

        // try using the requested kmer sizes
        let mut results = kmer_sizes
            .par_iter()
            .filter_map(|kmer_size| {
                self.create_graph(
                    reads,
                    ref_haplotype,
                    *kmer_size,
                    self.dont_increase_kmer_sizes_for_cycles,
                    self.allow_non_unique_kmers_in_ref,
                    sample_names,
                    dangling_end_sw_parameters,
                    avx_mode,
                )
                // {
                //     None => continue,
                //     Some(assembly_result) => {
                //         debug!(
                //             "Found assembly result (No increase) graph -> {:?}",
                //             assembly_result.graph.as_ref().unwrap().base_graph
                //         );
                //         results.push(assembly_result)
                //     }
                // }
            })
            .collect::<Vec<AssemblyResult<SimpleInterval, ReadThreadingGraph>>>();
        

        if results.is_empty() && !self.dont_increase_kmer_sizes_for_cycles {
            let mut kmer_size =
                *self.kmer_sizes.iter().max().unwrap() + Self::KMER_SIZE_ITERATION_INCREASE;
            // if kmer_size is even, add 1 to make it odd
            if kmer_size % 2 == 0 {
                kmer_size += 1;
            }
            let mut num_iterations = 1;
            while results.is_empty() && num_iterations <= Self::MAX_KMER_ITERATIONS_TO_ATTEMPT {
                // on the last attempt we will allow low complexity graphs
                let last_attempt = num_iterations == Self::MAX_KMER_ITERATIONS_TO_ATTEMPT;
                match self.create_graph(
                    reads,
                    &ref_haplotype,
                    kmer_size,
                    last_attempt,
                    last_attempt,
                    sample_names,
                    dangling_end_sw_parameters,
                    avx_mode,
                ) {
                    None => {
                        // pass
                    }
                    Some(assembly_result) => {
                        results.push(assembly_result);
                    }
                };
                kmer_size += Self::KMER_SIZE_ITERATION_INCREASE;
                num_iterations += 1;
            }
        }

        return results;
    }

    /**
     * Follow the kmer expansion heurisics as {@link #assemble(List, Haplotype, SAMFileHeader, SmithWatermanAligner)}, but in this case
     * attempt to recover haplotypes from the kmer graph and use them to assess whether to expand the kmer size.
     */
    fn assemble_graphs_and_expand_kmers_given_haplotypes<'b>(
        &mut self,
        ref_haplotype: &'b Haplotype<SimpleInterval>,
        ref_loc: &'b SimpleInterval,
        corrected_reads: &'b Vec<BirdToolRead>,
        result_set: &mut AssemblyResultSet<ReadThreadingGraph>,
        active_region_extended_location: &'b SimpleInterval,
        sample_names: &'b [String],
        dangling_end_sw_parameters: &Parameters,
        reference_to_haplotype_sw_parameters: &Parameters,
        avx_mode: AVXMode,
        additional_kmer_sizes: Option<Vec<usize>>
    ) {
        let mut saved_assembly_results = Vec::new();

        let mut has_adequately_assembled_graph = false;
        let kmers_to_try = self.get_expanded_kmer_list(additional_kmer_sizes);
        // first, try using the requested kmer sizes
        for i in 0..kmers_to_try.len() {
            let kmer_size = kmers_to_try[i];
            let is_last_cycle = i == kmers_to_try.len() - 1;
            if !has_adequately_assembled_graph {
                let assembled_result = self.create_graph(
                    corrected_reads,
                    &ref_haplotype,
                    kmer_size,
                    is_last_cycle || self.dont_increase_kmer_sizes_for_cycles,
                    is_last_cycle || self.allow_non_unique_kmers_in_ref,
                    &sample_names,
                    dangling_end_sw_parameters,
                    avx_mode,
                );
                match assembled_result {
                    None => {} //pass
                    Some(mut assembled_result) => {
                        if assembled_result.status == Status::AssembledSomeVariation {
                            // do some QC on the graph
                            Self::sanity_check_graph(
                                assembled_result
                                    .threading_graph
                                    .as_ref()
                                    .unwrap()
                                    .get_base_graph(),
                                &ref_haplotype,
                            );
                            let _ = &mut assembled_result
                                .threading_graph
                                .as_mut()
                                .unwrap()
                                .post_process_for_haplotype_finding(
                                    self.debug_graph_output_path.as_ref(),
                                    ref_haplotype.genome_location.as_ref().unwrap(),
                                );
                            // add it to graphs with meaningful non-reference features
                            // non_ref_rt_graphs.push(assembled_result.threading_graph.unwrap().clone());
                            // if graph
                            // TODO: Add histogram plotting
                            // let graph = assembled_result.threading_graph.as_ref().unwrap();
                            self.find_best_path(
                                &mut assembled_result,
                                ref_haplotype,
                                ref_loc,
                                active_region_extended_location,
                                reference_to_haplotype_sw_parameters,
                                result_set,
                                avx_mode,
                            );

                            saved_assembly_results.push(assembled_result);
                            //TODO LOGIC PLAN HERE - we want to check if we have a trustworthy graph (i.e. no badly assembled haplotypes) if we do, emit it.
                            //TODO                 - but if we failed to assemble due to excessive looping or did have badly assembled haplotypes then we expand kmer size.
                            //TODO                 - If we get no variation

                            // if asssembly didn't fail ( which is a degenerate case that occurs for some subset of graphs with difficult loop
                            if !saved_assembly_results
                                .last()
                                .unwrap()
                                .discovered_haplotypes
                                .is_empty()
                            {
                                // we have found our workable kmer size so lets add the results and finish
                                let assembled_result = saved_assembly_results.last().unwrap();
                                if !assembled_result.contains_suspect_haploptypes {
                                    // let mut result_set = result_set.lock().unwrap();
                                    for h in assembled_result.discovered_haplotypes.clone() {
                                        result_set.add_haplotype(h);
                                    }

                                    has_adequately_assembled_graph = true;
                                }
                            }
                        } else if assembled_result.status == Status::JustAssembledReference {
                            has_adequately_assembled_graph = true;
                        }
                    }
                }
            }
        }

        // This indicates that we have thrown everything away... we should go back and
        // check that we weren't too conservative about assembly results that might
        // otherwise be good
        if !has_adequately_assembled_graph {
            // search for the last haplotype set that had any results, if none are found just return me
            // In this case we prefer the last meaningful kmer size if possible
            // for result in saved_assembly_results.
            saved_assembly_results.reverse();
            for result in saved_assembly_results {
                if result.discovered_haplotypes.len() > 1 {
                    // let mut result_set = result_set.lock().unwrap();
                    let ar_index = result_set.add_assembly_result(result);
                    for h in result_set.assembly_results[ar_index]
                        .discovered_haplotypes
                        .clone()
                    {
                        result_set.add_haplotype_and_assembly_result(h, ar_index);
                    }
                    break;
                }
            }
        }
    }

    /**
     * Make sure the reference sequence is properly represented in the provided graph
     *
     * @param graph the graph to check
     * @param refHaplotype the reference haplotype
     */
    fn sanity_check_graph<'b, V: BaseVertex, E: BaseEdge>(
        graph: &'b BaseGraph<V, E>,
        ref_haplotype: &'b Haplotype<SimpleInterval>,
    ) {
        let ref_source_vertex = graph.get_reference_source_vertex();
        let ref_sink_vertex = graph.get_reference_sink_vertex();
        if ref_source_vertex.is_none() {
            panic!("All reference graphs must have a reference source vertex");
        };

        if ref_sink_vertex.is_none() {
            panic!("All reference graphs must have a reference sink vertex");
        };

        if graph.get_reference_bytes(ref_source_vertex.unwrap(), ref_sink_vertex, true, true)
            != ref_haplotype.get_bases()
        {
            panic!(
                "Mismatch between the reference haplotype and the reference assembly graph path for \n\
                 +++++++++ \n\
                 graph     = {} \n\
                 haplotype = {} \n\
                 loc       = {:?} \n\
                 +++++++++ \n",
                std::str::from_utf8(
                    graph
                        .get_reference_bytes(
                            ref_source_vertex.unwrap(),
                            ref_sink_vertex,
                            true,
                            true
                        )
                        .as_slice()
                )
                .unwrap(),
                std::str::from_utf8(ref_haplotype.get_bases()).unwrap(),
                &ref_haplotype.genome_location
            );
        };
    }

    /**
     * Method for getting a list of all of the specified kmer sizes to test for the graph including kmer expansions
     * @return
     */
    fn get_expanded_kmer_list(&self, additional_kmer_sizes: Option<Vec<usize>>) -> Vec<usize> {
        let mut return_list = Vec::new();
        return_list.extend(self.kmer_sizes.iter());
        if !self.dont_increase_kmer_sizes_for_cycles {
            let mut kmer_size =
                self.kmer_sizes.iter().max().unwrap() + Self::KMER_SIZE_ITERATION_INCREASE;
            let mut num_iterations = 1;
            while num_iterations <= Self::MAX_KMER_ITERATIONS_TO_ATTEMPT {
                return_list.push(kmer_size);
                kmer_size += Self::KMER_SIZE_ITERATION_INCREASE;
                num_iterations += 1;
            }
        }

        if let Some(additional_kmer_sizes) = additional_kmer_sizes {
            return_list.extend(additional_kmer_sizes.iter());
        }

        return return_list;
    }

    /**
     * Print graph to file NOTE this requires that debugGraphTransformations be enabled.
     *
     * @param graph the graph to print
     * @param fileName the name to give the graph file
     */
    fn print_debug_graph_transform_abstract<A: AbstractReadThreadingGraph>(
        &self,
        graph: &A,
        file_name: String,
    ) {
        // if Self::PRINT_FILL_GRAPH_FOR_DEBUGGING {
        //     graph.print_graph(file_name, self.prune_factor as usize)
        // } else {
        //     grap
        // }
        graph.print_graph(file_name, self.prune_factor as usize)
    }

    /**
     * Print graph to file NOTE this requires that debugGraphTransformations be enabled.
     *
     * @param graph the graph to print
     * @param fileName the name to give the graph file
     */
    fn print_debug_graph_transform_seq_graph<E: BaseEdge>(
        &self,
        graph: &SeqGraph<E>,
        file_name: String,
    ) {
        // if Self::PRINT_FILL_GRAPH_FOR_DEBUGGING {
        //     graph.print_graph(file_name, self.prune_factor as usize)
        // } else {
        //     grap
        // }
        graph
            .base_graph
            .print_graph(&file_name, true, self.prune_factor as usize)
    }

    /**
     * Find discover paths by using KBestHaplotypeFinder over each graph.
     *
     * This method has the side effect that it will annotate all of the AssemblyResults objects with the derived haplotypes
     * which can be used for basing kmer graph pruning on the discovered haplotypes.
     *
     * @param graph                 graph to be used for kmer detection
     * @param assemblyResults       assembly results objects for this graph
     * @param refHaplotype          reference haplotype
     * @param refLoc                location of reference haplotype
     * @param activeRegionWindow    window of the active region (without padding)
     * @param resultSet             (can be null) the results set into which to deposit discovered haplotypes
     * @param aligner               SmithWaterman aligner to use for aligning the discovered haplotype to the reference haplotype
     * @return A list of discovered haplotyes (note that this is not currently used for anything)
     */
    fn find_best_path<'b, A: AbstractReadThreadingGraph>(
        &self,
        // graph: &BaseGraph<V, E>,
        assembly_result: &'b mut AssemblyResult<SimpleInterval, A>,
        ref_haplotype: &'b Haplotype<SimpleInterval>,
        _ref_loc: &'b SimpleInterval,
        active_region_window: &'b SimpleInterval,
        haplotype_to_reference_sw_parameters: &Parameters,
        result_set: &mut AssemblyResultSet<A>,
        avx_mode: AVXMode,
    ) {
        // add the reference haplotype separately from all the others to ensure
        // that it is present in the list of haplotypes
        // let mut return_haplotypes = LinkedHashSet::new();
        let active_region_start = ref_haplotype.alignment_start_hap_wrt_ref;
        let mut failed_cigars = 0;
        {
            // Validate that the graph is valid with extant source and sink before operating
            let source = assembly_result
                .graph
                .as_ref()
                .unwrap()
                .base_graph
                .get_reference_source_vertex();
            let sink = assembly_result
                .graph
                .as_ref()
                .unwrap()
                .base_graph
                .get_reference_sink_vertex();
            assert!(
                source.is_some() && sink.is_some(),
                "Both source and sink cannot be null"
            );

            let k_best_haplotypes: Box<Vec<KBestHaplotype>> = if self.generate_seq_graph {
                Box::new(
                    GraphBasedKBestHaplotypeFinder::new_from_singletons(
                        &mut assembly_result.graph.as_mut().unwrap().base_graph,
                        source.unwrap(),
                        sink.unwrap(),
                    )
                    .find_best_haplotypes(
                        self.num_best_haplotypes_per_graph as usize,
                        &assembly_result.graph.as_ref().unwrap().base_graph,
                    ),
                )
            } else {
                // TODO: JunctionTreeKBestHaplotype looks munted and I haven't implemented the other
                //       JunctionTree stuff so skipping for now
                panic!("JunctionTree not yet supported, please set generate_seq_graph to true")
            };

            for k_best_haplotype in k_best_haplotypes.into_iter() {
                // TODO for now this seems like the solution, perhaps in the future it will be to excise the haplotype completely)
                // TODO: Lorikeet note, some weird Java shit happens here, will need a work around when
                //       junction tree is implemented
                let mut h =
                    k_best_haplotype.haplotype(&assembly_result.graph.as_ref().unwrap().base_graph);
                h.kmer_size = k_best_haplotype.kmer_size;

                if !result_set.haplotypes.contains(&h) {
                    // debug!(
                    //     "Potential location {:?} potential haplotype {:?}",
                    //     active_region_window, &h
                    // );
                    // TODO this score seems to be irrelevant at this point...
                    // if k_best_haplotype.is_reference {
                    //     ref_haplotype.score = OrderedFloat(k_best_haplotype.score);
                    // };

                    // debug!("+++++++++==================================== Candidates ====================================+++++++++");
                    // debug!(
                    //     "ref -> {}",
                    //     std::str::from_utf8(ref_haplotype.get_bases()).unwrap()
                    // );
                    // debug!("alt -> {}", std::str::from_utf8(h.get_bases()).unwrap());
                    // debug!("+++++++++====================================++++++++++++====================================+++++++++");
                    let cigar = CigarUtils::calculate_cigar(
                        ref_haplotype.get_bases(),
                        h.get_bases(),
                        OverhangStrategy::SoftClip,
                        haplotype_to_reference_sw_parameters,
                        avx_mode,
                    );

                    match cigar {
                        None => {
                            failed_cigars += 1;
                            continue;
                        }
                        Some(cigar) => {
                            if cigar.is_empty() {
                                panic!(
                                    "Smith-Waterman alignment failure. Cigar = {:?}, with reference \
                                length {} but expecting reference length of {}",
                                    &cigar, CigarUtils::get_reference_length(&cigar),
                                    CigarUtils::get_reference_length(&ref_haplotype.cigar)
                                )
                            } else if Self::path_is_too_divergent_from_reference(&cigar)
                                || CigarUtils::get_reference_length(&cigar)
                                    < Self::MIN_HAPLOTYPE_REFERENCE_LENGTH
                            {
                                // N cigar elements means that a bubble was too divergent from the reference so skip over this path
                                continue;
                            } else if CigarUtils::get_reference_length(&cigar)
                                != CigarUtils::get_reference_length(&ref_haplotype.cigar)
                            {
                                // the SOFTCLIP strategy can produce a haplotype cigar that matches the beginning of the reference and
                                // skips the latter part of the reference.  For example, when padded haplotype = NNNNNNNNNN[sequence 1]NNNNNNNNNN
                                // and padded ref = NNNNNNNNNN[sequence 1][sequence 2]NNNNNNNNNN, the alignment may choose to align only sequence 1.
                                // If aligning with an indel strategy produces a cigar with deletions for sequence 2 (which is reflected in the
                                // reference length of the cigar matching the reference length of the ref haplotype), then the assembly window was
                                // simply too small to reliably resolve the deletion; it should only throw an IllegalStateException when aligning
                                // with the INDEL strategy still produces discrepant reference lengths.
                                // You might wonder why not just use the INDEL strategy from the beginning.  This is because the SOFTCLIP strategy only fails
                                // when there is insufficient flanking sequence to resolve the cigar unambiguously.  The INDEL strategy would produce
                                // valid but most likely spurious indel cigars.
                                let cigar_with_indel_strategy = CigarUtils::calculate_cigar(
                                    ref_haplotype.get_bases(),
                                    h.get_bases(),
                                    OverhangStrategy::InDel,
                                    haplotype_to_reference_sw_parameters,
                                    avx_mode,
                                );

                                match cigar_with_indel_strategy {
                                    None => panic!("Smith-Waterman Alignment failure. No result"),
                                    Some(cigar_with_indel_strategy) => {
                                        if CigarUtils::get_reference_length(
                                            &cigar_with_indel_strategy,
                                        ) == CigarUtils::get_reference_length(
                                            &ref_haplotype.cigar,
                                        ) {
                                            failed_cigars += 1;
                                            continue;
                                        } else {
                                            panic!(
                                                "Smith-Waterman alignment failure. Cigar = {:?} with \
                                            reference length {} but expecting reference length of \
                                            {} ref = {:?} path = {:?}", &cigar,
                                                CigarUtils::get_reference_length(&cigar),
                                                CigarUtils::get_reference_length(&ref_haplotype.cigar),
                                                std::str::from_utf8(ref_haplotype.get_bases()),
                                                std::str::from_utf8(h.get_bases()),
                                            )
                                        }
                                    }
                                }
                            }

                            h.cigar = cigar;
                            h.alignment_start_hap_wrt_ref = active_region_start;
                            h.genome_location = Some(active_region_window.clone());
                            // debug!(
                            //     "Adding haplotype {:?} from graph with kmer {}",
                            //     &h.cigar,
                            //     assembly_result
                            //         .graph
                            //         .as_ref()
                            //         .unwrap()
                            //         .base_graph
                            //         .get_kmer_size()
                            // );
                            // return_haplotypes.insert(h.clone());
                            // result set would get added to here
                            // let mut result_set = result_set.lock().unwrap();
                            result_set.add_haplotype(h);
                        }
                    }
                }
            }
        }

        // Make sure that the ref haplotype is amongst the return haplotypes and calculate its score as
        // the first returned by any finder.
        // HERE we want to preserve the signal that assembly failed completely so in this case we don't add anything to the empty list
        // if !result_set.haplotypes.is_empty() && !result_set.haplotypes.contains(ref_haplotype) {
        //     return_haplotypes.insert(ref_haplotype.clone());
        // };

        if failed_cigars != 0 {
            // debug!(
            //     "Failed to align some haplotypes ({}) back to the reference (loc={:?}); \
            // these will be ignored",
            //     failed_cigars, ref_loc
            // )
        }

        // assembly_result.set_discovered_haplotypes(return_haplotypes);
    }

    /**
     * We use CigarOperator.N as the signal that an incomplete or too divergent bubble was found during bubble traversal
     * @param c the cigar to test
     * @return  true if we should skip over this path
     */
    fn path_is_too_divergent_from_reference(c: &CigarString) -> bool {
        return c.0.iter().any(|ce| match ce {
            Cigar::RefSkip(_) => true,
            _ => false,
        });
    }

    /**
     * Creates the sequence graph for the given kmerSize
     *
     * @param reads            reads to use
     * @param refHaplotype     reference haplotype
     * @param kmerSize         kmer size
     * @param allowLowComplexityGraphs if true, do not check for low-complexity graphs
     * @param allowNonUniqueKmersInRef if true, do not fail if the reference has non-unique kmers
     * @param aligner {@link SmithWatermanAligner} used to align dangling ends to the reference sequence
     * @return sequence graph or null if one could not be created (e.g. because it contains cycles or too many paths or is low complexity)
     */
    fn create_graph<'b>(
        &self,
        reads: &'b Vec<BirdToolRead>,
        ref_haplotype: &'b Haplotype<SimpleInterval>,
        kmer_size: usize,
        allow_low_complexity_graphs: bool,
        _allow_non_unique_kmers_in_ref: bool,
        sample_names: &'b [String],
        dangling_end_sw_parameters: &Parameters,
        avx_mode: AVXMode,
    ) -> Option<AssemblyResult<SimpleInterval, ReadThreadingGraph>> {
        if ref_haplotype.len() < kmer_size {
            // happens in cases where the assembled region is just too small
            return Some(AssemblyResult::new(Status::Failed, None, None));
        }

        if !self.allow_non_unique_kmers_in_ref
            && !ReadThreadingGraph::determine_non_unique_kmers(
                &SequenceForKmers::new(
                    "ref".to_string(),
                    ref_haplotype.get_bases(),
                    0,
                    ref_haplotype.get_bases().len(),
                    1,
                    true,
                ),
                kmer_size,
            )
            .is_empty()
        {
            // debug!("Not using kmer size of {kmer_size} in read threading assembler because reference contains non-unique kmers");
            return None;
        }

        let mut rt_graph =
        // if self.generate_seq_graph {
            ReadThreadingGraph::new(
                kmer_size,
                false,
                self.min_base_quality_to_use_in_assembly,
                self.num_pruning_samples as usize,
                self.min_matching_bases_to_dangling_end_recovery,
                avx_mode
            );
        // } else {
        //     // This is where the junction tree debruijn graph would go but considering it is experimental
        //     // we will leave it out for now
        //     ReadThreadingGraph::new(
        //         kmer_size,
        //         false,
        //         self.min_base_quality_to_use_in_assembly,
        //         self.num_pruning_samples as usize,
        //         self.min_matching_bases_to_dangling_end_recovery,
        //     )
        // };

        rt_graph.set_threading_start_only_at_existing_vertex(!self.recover_dangling_branches);

        // add the reference sequence to the graph
        let mut pending = LinkedHashMap::new();
        rt_graph.add_sequence(
            &mut pending,
            "ref".to_string(),
            // ReadThreadingGraph::ANONYMOUS_SAMPLE,
            std::usize::MAX,
            ref_haplotype.get_bases(),
            0,
            ref_haplotype.get_bases().len(),
            1,
            true,
        );
        // debug!(
        //     "1 - Graph Kmer {} Edges {} Nodes {}",
        //     kmer_size,
        //     rt_graph.base_graph.graph.edge_count(),
        //     rt_graph.base_graph.graph.node_count()
        // );

        // Next pull kmers out of every read and throw them on the graph
        // debug!("1.5 - Reads {}", reads.len());
        let mut count = 0;

        let mut sample_count = LinkedHashMap::new();
        // let mut read_debugging = false;
        for read in reads {
            let s_count = sample_count.entry(read.sample_index).or_insert(0);
            *s_count += 1;
            // if read.name() == b"DFDW01000005.1-5" {
            //     // debug!("Read {:?}", read);
            //     read_debugging = true;
            // };
            rt_graph.add_read(read, sample_names, &mut count, &mut pending)
        }
        // debug!("1.5 - Count {} -> {:?}", count, sample_count);
        // let pending = rt_graph.get_pending(); // retrieve pending sequences and clear pending from graph
        // actually build the read threading graph
        rt_graph.build_graph_if_necessary(&mut pending);
        // debug!(
        //     "2 - Graph Kmer {} Edges {} Nodes {}",
        //     kmer_size,
        //     rt_graph.base_graph.graph.edge_count(),
        //     rt_graph.base_graph.graph.node_count()
        // );

        if self.debug_graph_transformations {
            self.print_debug_graph_transform_abstract(
                &rt_graph,
                format!(
                    "{}_{}-{}-sequenceGraph.{}.0.0.raw_threading_graph.dot",
                    ref_haplotype.genome_location.as_ref().unwrap().tid(),
                    ref_haplotype.genome_location.as_ref().unwrap().get_start(),
                    ref_haplotype.genome_location.as_ref().unwrap().get_end(),
                    kmer_size
                ),
            )
        }

        // It's important to prune before recovering dangling ends so that we don't waste time recovering bad ends.
        // It's also important to prune before checking for cycles so that sequencing errors don't create false cycles
        // and unnecessarily abort assembly
        if self.prune_before_cycle_counting {
            self.chain_pruner
                .prune_low_weight_chains(rt_graph.get_base_graph_mut());
        }
        // debug!(
        //     "3 - Graph Kmer {} Edges {} Nodes {}",
        //     kmer_size,
        //     rt_graph.base_graph.graph.edge_count(),
        //     rt_graph.base_graph.graph.node_count()
        // );

        // sanity check: make sure there are no cycles in the graph, unless we are in experimental mode
        if self.generate_seq_graph && rt_graph.has_cycles() {
            // debug!(
            //     "Not using kmer size of {}  in read threading assembler \
            //         because it contains a cycle",
            //     kmer_size
            // );
            return None;
        }

        // sanity check: make sure the graph had enough complexity with the given kmer
        if !allow_low_complexity_graphs && rt_graph.is_low_quality_graph() {
            // debug!(
            //     "Not using kmer size of {} in read threading assembler because it does not \
            //         produce a graph with enough complexity",
            //     kmer_size
            // );
            return None;
        }

        let result = self.get_assembly_result(
            ref_haplotype,
            kmer_size,
            rt_graph,
            dangling_end_sw_parameters,
        );
        // check whether recovering dangling ends created cycles
        if self.recover_all_dangling_branches
            && result.threading_graph.as_ref().unwrap().has_cycles()
        {
            return None;
        }

        return Some(result);
    }

    fn get_assembly_result<A: AbstractReadThreadingGraph>(
        &self,
        ref_haplotype: &Haplotype<SimpleInterval>,
        kmer_size: usize,
        mut rt_graph: A,
        dangling_end_sw_parameters: &Parameters,
    ) -> AssemblyResult<SimpleInterval, A> {
        if !self.prune_before_cycle_counting {
            self.chain_pruner
                .prune_low_weight_chains(rt_graph.get_base_graph_mut())
        }

        if self.debug_graph_transformations {
            self.print_debug_graph_transform_abstract(
                &rt_graph,
                format!(
                    "{}_{}-{}-sequenceGraph.{}.0.1.chain_pruned_readthreading_graph.dot",
                    ref_haplotype.genome_location.as_ref().unwrap().tid(),
                    ref_haplotype.genome_location.as_ref().unwrap().get_start(),
                    ref_haplotype.genome_location.as_ref().unwrap().get_end(),
                    kmer_size
                ),
            );
        };

        // look at all chains in the graph that terminate in a non-ref node (dangling sources and sinks) and see if
        // we can recover them by merging some N bases from the chain back into the reference
        if self.recover_dangling_branches {
            rt_graph.recover_dangling_tails(
                self.prune_factor as usize,
                self.min_dangling_branch_length,
                self.recover_all_dangling_branches,
                dangling_end_sw_parameters,
            );
            rt_graph.recover_dangling_heads(
                self.prune_factor as usize,
                self.min_dangling_branch_length,
                self.recover_all_dangling_branches,
                dangling_end_sw_parameters,
            );
        }

        // remove all heading and trailing paths
        if self.remove_paths_not_connected_to_ref {
            rt_graph.remove_paths_not_connected_to_ref()
        }

        if self.debug_graph_transformations {
            self.print_debug_graph_transform_abstract(
                &rt_graph,
                format!(
                    "{}_{}-{}-sequenceGraph.{}.0.2.cleaned_readthreading_graph.dot",
                    ref_haplotype.genome_location.as_ref().unwrap().tid(),
                    ref_haplotype.genome_location.as_ref().unwrap().get_start(),
                    ref_haplotype.genome_location.as_ref().unwrap().get_end(),
                    kmer_size
                ),
            );
        };

        // Either return an assembly result with a sequence graph or with an unchanged
        // sequence graph deptending on the kmer duplication behavior
        if self.generate_seq_graph {
            let mut initial_seq_graph = rt_graph.to_sequence_graph();

            if self.debug_graph_transformations {
                rt_graph.print_graph(
                    format!(
                        "{}_{}-{}-sequenceGraph.{}.0.3.initial_seqgraph.dot",
                        ref_haplotype.genome_location.as_ref().unwrap().tid(),
                        ref_haplotype.genome_location.as_ref().unwrap().get_start(),
                        ref_haplotype.genome_location.as_ref().unwrap().get_end(),
                        kmer_size
                    ),
                    10000,
                );
            };

            // if the unit tests don't want us to cleanup the graph, just return the raw sequence graph
            if self.just_return_raw_graph {
                return AssemblyResult::new(
                    Status::AssembledSomeVariation,
                    Some(initial_seq_graph),
                    None,
                );
            }

            // debug!(
            //     "Using kmer size of {} in read threading assembler",
            //     &rt_graph.get_kmer_size()
            // );

            if self.debug_graph_transformations {
                self.print_debug_graph_transform_abstract(
                    &rt_graph,
                    format!(
                        "{}_{}-{}-sequenceGraph.{}.0.4.initial_seqgraph.dot",
                        ref_haplotype.genome_location.as_ref().unwrap().tid(),
                        ref_haplotype.genome_location.as_ref().unwrap().get_start(),
                        ref_haplotype.genome_location.as_ref().unwrap().get_end(),
                        kmer_size
                    ),
                );
            };

            initial_seq_graph.base_graph.clean_non_ref_paths();
            let cleaned: AssemblyResult<SimpleInterval, ReadThreadingGraph> =
                self.clean_up_seq_graph(initial_seq_graph, &ref_haplotype);
            let status = cleaned.status;
            return AssemblyResult::new(status, cleaned.graph, Some(rt_graph));
        } else {
            // if the unit tests don't want us to cleanup the graph, just return the raw sequence graph
            if self.just_return_raw_graph {
                return AssemblyResult::new(Status::AssembledSomeVariation, None, Some(rt_graph));
            }

            // debug!(
            //     "Using kmer size of {} in read threading assembler",
            //     &rt_graph.get_kmer_size()
            // );
            let cleaned = Self::get_result_set_for_rt_graph(rt_graph);
            return cleaned;
        }
    }

    fn get_result_set_for_rt_graph<A: AbstractReadThreadingGraph>(
        rt_graph: A,
    ) -> AssemblyResult<SimpleInterval, A> {
        // The graph has degenerated in some way, so the reference source and/or sink cannot be id'd.  Can
        // happen in cases where for example the reference somehow manages to acquire a cycle, or
        // where the entire assembly collapses back into the reference sequence.
        if rt_graph.get_reference_source_vertex().is_none()
            || rt_graph.get_reference_sink_vertex().is_none()
        {
            return AssemblyResult::new(Status::JustAssembledReference, None, Some(rt_graph));
        };

        return AssemblyResult::new(Status::AssembledSomeVariation, None, Some(rt_graph));
    }

    // Performs the various transformations necessary on a sequence graph
    fn clean_up_seq_graph<A: AbstractReadThreadingGraph>(
        &self,
        mut seq_graph: SeqGraph<BaseEdgeStruct>,
        ref_haplotype: &Haplotype<SimpleInterval>,
    ) -> AssemblyResult<SimpleInterval, A> {
        if self.debug_graph_transformations {
            self.print_debug_graph_transform_seq_graph(
                &seq_graph,
                format!(
                    "{}_{}-{}-sequenceGraph.{}.1.0.non_ref_removed.dot",
                    ref_haplotype.genome_location.as_ref().unwrap().tid(),
                    ref_haplotype.genome_location.as_ref().unwrap().get_start(),
                    ref_haplotype.genome_location.as_ref().unwrap().get_end(),
                    seq_graph.base_graph.get_kmer_size()
                ),
            );
        };

        // the very first thing we need to do is zip up the graph, or pruneGraph will be too aggressive
        seq_graph.zip_linear_chains();
        if self.debug_graph_transformations {
            self.print_debug_graph_transform_seq_graph(
                &seq_graph,
                format!(
                    "{}_{}-{}-sequenceGraph.{}.1.1.zipped.dot",
                    ref_haplotype.genome_location.as_ref().unwrap().tid(),
                    ref_haplotype.genome_location.as_ref().unwrap().get_start(),
                    ref_haplotype.genome_location.as_ref().unwrap().get_end(),
                    seq_graph.base_graph.get_kmer_size()
                ),
            );
        };

        // now go through and prune the graph, removing vertices no longer connected to the reference chain
        seq_graph.base_graph.remove_singleton_orphan_vertices();
        seq_graph
            .base_graph
            .remove_vertices_not_connected_to_ref_regardless_of_edge_direction();

        if self.debug_graph_transformations {
            self.print_debug_graph_transform_seq_graph(
                &seq_graph,
                format!(
                    "{}_{}-{}-sequenceGraph.{}.1.2.pruned.dot",
                    ref_haplotype.genome_location.as_ref().unwrap().tid(),
                    ref_haplotype.genome_location.as_ref().unwrap().get_start(),
                    ref_haplotype.genome_location.as_ref().unwrap().get_end(),
                    seq_graph.base_graph.get_kmer_size()
                ),
            );
        };

        seq_graph.simplify_graph(&format!(
            "{}_{}-{}.0",
            ref_haplotype.genome_location.as_ref().unwrap().tid(),
            ref_haplotype.genome_location.as_ref().unwrap().get_start(),
            ref_haplotype.genome_location.as_ref().unwrap().get_end(),
        ));
        if self.debug_graph_transformations {
            self.print_debug_graph_transform_seq_graph(
                &seq_graph,
                format!(
                    "{}_{}-{}-sequenceGraph.{}.1.3.merged.dot",
                    ref_haplotype.genome_location.as_ref().unwrap().tid(),
                    ref_haplotype.genome_location.as_ref().unwrap().get_start(),
                    ref_haplotype.genome_location.as_ref().unwrap().get_end(),
                    seq_graph.base_graph.get_kmer_size()
                ),
            );
        };

        // The graph has degenerated in some way, so the reference source and/or sink cannot be id'd.  Can
        // happen in cases where for example the reference somehow manages to acquire a cycle, or
        // where the entire assembly collapses back into the reference sequence.
        if seq_graph.base_graph.get_reference_source_vertex().is_none()
            || seq_graph.base_graph.get_reference_sink_vertex().is_none()
        {
            return AssemblyResult::new(Status::JustAssembledReference, Some(seq_graph), None);
        };

        seq_graph.base_graph.remove_paths_not_connected_to_ref();
        seq_graph.simplify_graph(&format!(
            "{}_{}-{}.1",
            ref_haplotype.genome_location.as_ref().unwrap().tid(),
            ref_haplotype.genome_location.as_ref().unwrap().get_start(),
            ref_haplotype.genome_location.as_ref().unwrap().get_end(),
        ));
        if seq_graph.base_graph.graph.node_indices().count() == 1 {
            // we've perfectly assembled into a single reference haplotype, add a empty seq vertex to stop
            // the code from blowing up.
            // TODO -- ref properties should really be on the vertices, not the graph itself
            let complete = seq_graph.base_graph.graph.node_indices().next().unwrap();
            let dummy = SeqVertex::new(Vec::new());
            let dummy_index = seq_graph.base_graph.add_node(&dummy);
            seq_graph.base_graph.graph.add_edge(
                complete,
                dummy_index,
                BaseEdgeStruct::new(true, 0, 0),
            );
        };

        if self.debug_graph_transformations {
            self.print_debug_graph_transform_seq_graph(
                &seq_graph,
                format!(
                    "{}_{}-{}-sequenceGraph.{}.1.4.final.dot",
                    ref_haplotype.genome_location.as_ref().unwrap().tid(),
                    ref_haplotype.genome_location.as_ref().unwrap().get_start(),
                    ref_haplotype.genome_location.as_ref().unwrap().get_end(),
                    seq_graph.base_graph.get_kmer_size(),
                ),
            );
        };

        return AssemblyResult::new(Status::AssembledSomeVariation, Some(seq_graph), None);
    }

    // fn print_seq_graphs(&self, graphs: &Vec<SeqGraph<BaseEdgeStruct>>) {
    //     let _write_first_graph_with_size_smaller_than = 50;

    //     for (idx, graph) in graphs.iter().enumerate() {
    //         graph.base_graph.print_graph(
    //             &(self.graph_output_path.as_ref().unwrap().to_string() + &idx.to_string()),
    //             false,
    //             self.prune_factor as usize,
    //         )
    //     }
    // }
}
