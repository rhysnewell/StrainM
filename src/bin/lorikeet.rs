extern crate openssl;
extern crate openssl_sys;

extern crate lorikeet_genome;
use lorikeet_genome::cli::*;
use lorikeet_genome::external_command_checker;
use reference::reference_reader_utils::GenomesAndContigs;
use lorikeet_genome::utils::utils::*;
use lorikeet_genome::*;

extern crate coverm;
use coverm::bam_generator::*;
use coverm::mosdepth_genome_coverage_estimators::*;
use coverm::FlagFilter;
use coverm::*;

extern crate bird_tool_utils;

use std::env;
use std::process;
extern crate tempfile;
use tempfile::NamedTempFile;

extern crate clap;
use clap::*;

extern crate clap_complete;
use clap_complete::{generate, Shell};

#[macro_use]
extern crate log;
use log::LevelFilter;
extern crate env_logger;
use env_logger::Builder;
use lorikeet_genome::processing::lorikeet_engine::{
    run_summarize, start_lorikeet_engine, ReadType
};
use lorikeet_genome::reference::reference_reader_utils::ReferenceReaderUtils;
use lorikeet_genome::utils::errors::BirdToolError;

fn main() {
    let mut app = build_cli();
    let matches = app.clone().get_matches();
    set_log_level(&matches, false);

    match matches.subcommand_name() {
        Some("summarise") => {
            let m = matches.subcommand_matches("summarise").unwrap();
            bird_tool_utils::clap_utils::print_full_help_if_needed(&m, summarise_full_help());
            rayon::ThreadPoolBuilder::new()
                .num_threads(m.value_of("threads").unwrap().parse().unwrap())
                .build_global()
                .unwrap();
            run_summarize(m);
        }
        Some("genotype") => {
            let m = matches.subcommand_matches("genotype").unwrap();
            bird_tool_utils::clap_utils::print_full_help_if_needed(&m, genotype_full_help());
            let mode = "genotype";

            match prepare_pileup(m, mode) {
                Ok(_) => info!("Genotype complete."),
                Err(e) => warn!("Genotype failed with error: {:?}", e),
            };
        }
        Some("call") => {
            let m = matches.subcommand_matches("call").unwrap();
            bird_tool_utils::clap_utils::print_full_help_if_needed(&m, call_full_help());
            let mode = "call";

            match prepare_pileup(m, mode) {
                Ok(_) => info!("Call complete."),
                Err(e) => warn!("Call failed with error: {:?}", e),
            };
        }
        Some("consensus") => {
            let m = matches.subcommand_matches("consensus").unwrap();
            bird_tool_utils::clap_utils::print_full_help_if_needed(&m, consensus_full_help());
            let mode = "consensus";

            match prepare_pileup(m, mode) {
                Ok(_) => info!("Consensus complete."),
                Err(e) => warn!("Consensus failed with error: {:?}", e),
            };
        }
        Some("shell-completion") => {
            let m = matches.subcommand_matches("shell-completion").unwrap();
            set_log_level(m, true);
            let mut file = std::fs::File::create(m.value_of("output-file").unwrap())
                .expect("failed to open output file");

            if let Some(generator) = m.get_one::<Shell>("shell").copied() {
                let mut cmd = build_cli();
                info!("Generating completion script for shell {}", generator);
                let name = cmd.get_name().to_string();
                generate(generator, &mut cmd, name, &mut file);
            }
        }
        _ => {
            app.print_help().unwrap();
            println!();
        }
    }
}

fn prepare_pileup(m: &clap::ArgMatches, mode: &str) -> Result<(), BirdToolError> {
    // This function is amazingly painful. It handles every combination of longread and short read
    // mapping or bam file reading. Could not make it smaller using dynamic or static dispatch
    set_log_level(m, true);
    let mut estimators = EstimatorsAndTaker::generate_from_clap(m);
    let filter_params = FilterParameters::generate_from_clap(m);
    let threads = m.value_of("threads").unwrap().parse().unwrap();
    rayon::ThreadPoolBuilder::new()
        .num_threads(threads)
        .build_global()
        .unwrap();

    let references = ReferenceReaderUtils::parse_references(m);
    let references = references.iter().map(|p| &**p).collect::<Vec<&str>>();

    // Temp directory that will house all cached bams for variant calling
    let tmp_dir = match m.is_present("bam-file-cache-directory") {
        false => {
            let tmp_direct = tempdir::TempDir::new("lorikeet_fifo")
                .expect("Unable to create temporary directory");
            debug!("Temp directory {}", tmp_direct.as_ref().to_str().unwrap());
            std::fs::create_dir(format!("{}/long", &tmp_direct.as_ref().to_str().unwrap()))
                .unwrap();
            std::fs::create_dir(format!("{}/short", &tmp_direct.as_ref().to_str().unwrap()))
                .unwrap();
            std::fs::create_dir(format!(
                "{}/assembly",
                &tmp_direct.as_ref().to_str().unwrap()
            ))
            .unwrap();

            Some(tmp_direct)
        }
        true => None,
    };

    let (concatenated_genomes, genomes_and_contigs_option) =
        ReferenceReaderUtils::setup_genome_fasta_files(&m);
    debug!("Found genomes_and_contigs {:?}", genomes_and_contigs_option);
    if m.is_present("bam-files") {
        let bam_files: Vec<&str> = m.values_of("bam-files").unwrap().collect();

        // Associate genomes and contig names, if required
        if filter_params.doing_filtering() {
            let bam_readers = bam_generator::generate_filtered_bam_readers_from_bam_files(
                bam_files,
                filter_params.flag_filters.clone(),
                filter_params.min_aligned_length_single,
                filter_params.min_percent_identity_single,
                filter_params.min_aligned_percent_single,
                filter_params.min_aligned_length_pair,
                filter_params.min_percent_identity_pair,
                filter_params.min_aligned_percent_pair,
            );

            if m.is_present("longread-bam-files") {
                let bam_files = m.values_of("longread-bam-files").unwrap().collect();
                let long_readers =
                    bam_generator::generate_named_bam_readers_from_bam_files(bam_files);
                return run_pileup(
                    m,
                    mode,
                    &mut estimators,
                    bam_readers,
                    filter_params.flag_filters,
                    Some(long_readers),
                    genomes_and_contigs_option,
                    tmp_dir,
                    concatenated_genomes,
                );
            } else if m.is_present("longreads") {
                // Perform mapping
                let (long_generators, _indices) = long_generator_setup(
                    &m,
                    &concatenated_genomes,
                    &Some(references.clone()),
                    &tmp_dir,
                );

                return run_pileup(
                    m,
                    mode,
                    &mut estimators,
                    bam_readers,
                    filter_params.flag_filters,
                    Some(long_generators),
                    genomes_and_contigs_option,
                    tmp_dir,
                    concatenated_genomes,
                );
            } else {
                return run_pileup(
                    m,
                    mode,
                    &mut estimators,
                    bam_readers,
                    filter_params.flag_filters,
                    None::<Vec<PlaceholderBamFileReader>>,
                    genomes_and_contigs_option,
                    tmp_dir,
                    concatenated_genomes,
                );
            }
        } else {
            let bam_readers = bam_generator::generate_named_bam_readers_from_bam_files(bam_files);

            if m.is_present("longread-bam-files") {
                let bam_files = m.values_of("longread-bam-files").unwrap().collect();
                let long_readers =
                    bam_generator::generate_named_bam_readers_from_bam_files(bam_files);
                return run_pileup(
                    m,
                    mode,
                    &mut estimators,
                    bam_readers,
                    filter_params.flag_filters,
                    Some(long_readers),
                    genomes_and_contigs_option,
                    tmp_dir,
                    concatenated_genomes,
                );
            } else if m.is_present("longreads") {
                // Perform mapping
                let (long_generators, _indices) = long_generator_setup(
                    &m,
                    &concatenated_genomes,
                    &Some(references.clone()),
                    &tmp_dir,
                );

                return run_pileup(
                    m,
                    mode,
                    &mut estimators,
                    bam_readers,
                    filter_params.flag_filters,
                    Some(long_generators),
                    genomes_and_contigs_option,
                    tmp_dir,
                    concatenated_genomes,
                );
            } else {
                return run_pileup(
                    m,
                    mode,
                    &mut estimators,
                    bam_readers,
                    filter_params.flag_filters,
                    None::<Vec<PlaceholderBamFileReader>>,
                    genomes_and_contigs_option,
                    tmp_dir,
                    concatenated_genomes,
                );
            }
        }
    } else {
        let mapping_program = parse_mapping_program(m.value_of("mapper"));
        external_command_checker::check_for_samtools();

        if filter_params.doing_filtering() {
            debug!("Filtering..");
            let readtype = ReadType::Short;
            let generator_sets = get_streamed_filtered_bam_readers(
                m,
                mapping_program,
                &concatenated_genomes,
                &filter_params,
                &readtype,
                &Some(references.clone()),
                &tmp_dir,
            );
            let mut all_generators = vec![];
            let mut indices = vec![]; // Prevent indices from being dropped
            for set in generator_sets {
                indices.push(set.index);
                for g in set.generators {
                    all_generators.push(g)
                }
            }
            debug!("Finished collecting generators.");
            if m.is_present("longread-bam-files") {
                let bam_files = m.values_of("longread-bam-files").unwrap().collect();
                let long_readers =
                    bam_generator::generate_named_bam_readers_from_bam_files(bam_files);
                return run_pileup(
                    m,
                    mode,
                    &mut estimators,
                    all_generators,
                    filter_params.flag_filters,
                    Some(long_readers),
                    genomes_and_contigs_option,
                    tmp_dir,
                    concatenated_genomes,
                );
            } else if m.is_present("longreads") {
                // Perform mapping
                let (long_generators, _indices) = long_generator_setup(
                    &m,
                    &concatenated_genomes,
                    &Some(references.clone()),
                    &tmp_dir,
                );

                return run_pileup(
                    m,
                    mode,
                    &mut estimators,
                    all_generators,
                    filter_params.flag_filters,
                    Some(long_generators),
                    genomes_and_contigs_option,
                    tmp_dir,
                    concatenated_genomes,
                );
            } else {
                return run_pileup(
                    m,
                    mode,
                    &mut estimators,
                    all_generators,
                    filter_params.flag_filters,
                    None::<Vec<PlaceholderBamFileReader>>,
                    genomes_and_contigs_option,
                    tmp_dir,
                    concatenated_genomes,
                );
            }
        } else {
            debug!("Not filtering..");
            let readtype = ReadType::Short;
            let generator_sets = get_streamed_bam_readers(
                m,
                mapping_program,
                &concatenated_genomes,
                &readtype,
                &Some(references.clone()),
                &tmp_dir,
            );
            let mut all_generators = vec![];
            let mut indices = vec![]; // Prevent indices from being dropped
            for set in generator_sets {
                indices.push(set.index);
                for g in set.generators {
                    all_generators.push(g)
                }
            }

            if m.is_present("longread-bam-files") {
                let bam_files = m.values_of("longread-bam-files").unwrap().collect();
                let long_readers =
                    bam_generator::generate_named_bam_readers_from_bam_files(bam_files);

                return run_pileup(
                    m,
                    mode,
                    &mut estimators,
                    all_generators,
                    filter_params.flag_filters,
                    Some(long_readers),
                    genomes_and_contigs_option,
                    tmp_dir,
                    concatenated_genomes,
                );
            } else if m.is_present("longreads") {
                // Perform mapping
                let (long_generators, _indices) = long_generator_setup(
                    &m,
                    &concatenated_genomes,
                    &Some(references.clone()),
                    &tmp_dir,
                );

                return run_pileup(
                    m,
                    mode,
                    &mut estimators,
                    all_generators,
                    filter_params.flag_filters,
                    Some(long_generators),
                    genomes_and_contigs_option,
                    tmp_dir,
                    concatenated_genomes,
                );
            } else {
                return run_pileup(
                    m,
                    mode,
                    &mut estimators,
                    all_generators,
                    filter_params.flag_filters,
                    None::<Vec<PlaceholderBamFileReader>>,
                    genomes_and_contigs_option,
                    tmp_dir,
                    concatenated_genomes,
                );
            }
        }
    }
}

struct EstimatorsAndTaker {
    estimators: Vec<CoverageEstimator>,
}

impl EstimatorsAndTaker {
    pub fn generate_from_clap(m: &clap::ArgMatches) -> EstimatorsAndTaker {
        let mut estimators = vec![];
        let min_fraction_covered = parse_percentage(&m, "min-covered-fraction");
        let contig_end_exclusion = value_t!(m.value_of("contig-end-exclusion"), u64).unwrap();

        let methods: Vec<&str> = m.values_of("method").unwrap().collect();

        if doing_metabat(&m) {
            estimators.push(CoverageEstimator::new_estimator_length());
            estimators.push(CoverageEstimator::new_estimator_mean(
                min_fraction_covered,
                contig_end_exclusion,
                false,
            ));
            estimators.push(CoverageEstimator::new_estimator_variance(
                min_fraction_covered,
                contig_end_exclusion,
            ));

            debug!("Cached regular coverage taker for metabat mode being used");
        } else {
            for (_i, method) in methods.iter().enumerate() {
                match method {
                    &"mean" => {
                        estimators.push(CoverageEstimator::new_estimator_length());

                        estimators.push(CoverageEstimator::new_estimator_mean(
                            min_fraction_covered,
                            contig_end_exclusion,
                            false,
                        )); // TODO: Parameterise exclude_mismatches

                        estimators.push(CoverageEstimator::new_estimator_variance(
                            min_fraction_covered,
                            contig_end_exclusion,
                        ));
                    }
                    &"trimmed_mean" => {
                        let min = value_t!(m.value_of("trim-min"), f32).unwrap();
                        let max = value_t!(m.value_of("trim-max"), f32).unwrap();
                        if min < 0.0 || min > 1.0 || max <= min || max > 1.0 {
                            error!(
                                "error: Trim bounds must be between 0 and 1, and \
                                 min must be less than max, found {} and {}",
                                min, max
                            );
                            process::exit(1);
                        }
                        estimators.push(CoverageEstimator::new_estimator_length());

                        estimators.push(CoverageEstimator::new_estimator_trimmed_mean(
                            min,
                            max,
                            min_fraction_covered,
                            contig_end_exclusion,
                        ));

                        estimators.push(CoverageEstimator::new_estimator_variance(
                            min_fraction_covered,
                            contig_end_exclusion,
                        ));
                    }
                    _ => unreachable!(),
                };
            }
        }

        // Check that min-covered-fraction is being used as expected
        if min_fraction_covered != 0.0 {
            let die = |estimator_name| {
                error!(
                    "The '{}' coverage estimator cannot be used when \
                     --min-covered-fraction is > 0 as it does not calculate \
                     the covered fraction. You may wish to set the \
                     --min-covered-fraction to 0 and/or run this estimator \
                     separately.",
                    estimator_name
                );
                process::exit(1)
            };
            for e in &estimators {
                match e {
                    CoverageEstimator::ReadCountCalculator { .. } => die("counts"),
                    CoverageEstimator::ReferenceLengthCalculator { .. } => die("length"),
                    CoverageEstimator::ReadsPerBaseCalculator { .. } => die("reads_per_base"),
                    _ => {}
                }
            }
        }

        return EstimatorsAndTaker {
            estimators: estimators,
        };
    }
}

fn run_pileup<
    'a,
    R: NamedBamReader,
    S: NamedBamReaderGenerator<R>,
    T: NamedBamReader,
    U: NamedBamReaderGenerator<T>,
>(
    m: &clap::ArgMatches,
    mode: &str,
    estimators: &mut EstimatorsAndTaker,
    bam_readers: Vec<S>,
    flag_filters: FlagFilter,
    long_readers: Option<Vec<U>>,
    genomes_and_contigs_option: Option<GenomesAndContigs>,
    tmp_bam_file_cache: Option<tempdir::TempDir>,
    concatenated_genomes: Option<NamedTempFile>,
) -> Result<(), BirdToolError> {
    let genomes_and_contigs = genomes_and_contigs_option.unwrap();

    start_lorikeet_engine(
        m,
        bam_readers,
        long_readers,
        mode,
        estimators.estimators.clone(),
        flag_filters,
        genomes_and_contigs,
        tmp_bam_file_cache,
        concatenated_genomes,
    )?;
    Ok(())
}

fn set_log_level(matches: &clap::ArgMatches, is_last: bool) {
    let mut log_level = LevelFilter::Info;
    let mut specified = false;
    if matches.is_present("verbose") {
        specified = true;
        log_level = LevelFilter::Debug;
    }
    if matches.is_present("quiet") {
        specified = true;
        log_level = LevelFilter::Error;
    }
    if specified || is_last {
        let mut builder = Builder::new();
        builder.filter_level(log_level);
        if env::var("RUST_LOG").is_ok() {
            builder.parse_filters(&env::var("RUST_LOG").unwrap());
        }
        if builder.try_init().is_err() {
            panic!("Failed to set log level - has it been specified multiple times?")
        }
    }
    if is_last {
        info!("lorikeet version {}", crate_version!());
    }
}
