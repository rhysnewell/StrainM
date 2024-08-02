---
title: Lorikeet genotype
---
# lorikeet genotype

# DESCRIPTION

EXPERIMENTAL!

lorikeet genotype discovers variants within a given set of reads and
genomes and clusters the variants into candidate strain haplotypes.
Lorikeet uses UMAP and HDBSCAN to cluster variants and an
Expectation-Maximization algorithm to determine strain haplotype
abudnances within each samples. Additionally, calculate strain diversity
metrics like conANI, popANI, subpopANI, dN/dS, and the highly robust
Hudson\'s Fst

This process can be undertaken in several ways, for instance by
specifying BAM files or raw reads as input, using different mapping
programs, thresholding read alignments.

# FLAGS

**-v**, **\--verbose**

  Print extra debugging information. [default: not set]

**-q**, **\--quiet**

  Unless there is an error, do not print log messages. [default: not
    set]

# THREADING OPTIONS

**-t**, **\--threads** *INT*

  Maximum number of threads used. [default: 10]

**-P**, **\--parallel-genomes** *INT*

  Number of genomes to run in parallel. Increases memory usage
    linearly. Thread usage qill not exceed the value provided by
    \--threads [default 1]

# INPUT REFERENCE OPTIONS

**-r,-f**, **\--reference,\--genome-fasta-files** *PATH*

  FASTA files of contigs e.g. concatenated genomes or metagenome
    assembly [required unless `-d/--genome-fasta-directory` is
    specified]

**-d**, **\--genome-fasta-directory** *PATH*

  Directory containing FASTA files of contigs e.g. genomes or
    metagenome assembly [required unless
    `-r/--reference/-f/--genome-fasta-files` is specified]

**-x**, **\--genome-fasta-extension** *STR*

  FASTA file extension in \--genome-fasta-directory [default
    \"fna\"]

# READ MAPPING PARAMETERS

**-1** *PATH ..*

  Forward FASTA/Q file(s) for mapping. These may be gzipped or not.

**-2** *PATH ..*

  Reverse FASTA/Q file(s) for mapping. These may be gzipped or not.

**-c**, **\--coupled** *PATH ..*

  One or more pairs of forward and reverse possibly gzipped FASTA/Q
    files for mapping in order \<sample1_R1.fq.gz\> \<sample1_R2.fq.gz\>
    \<sample2_R1.fq.gz\> \<sample2_R2.fq.gz\> ..

**\--interleaved** *PATH ..*

  Interleaved FASTA/Q files(s) for mapping. These may be gzipped or
    not.

**\--single** *PATH ..*

  Unpaired FASTA/Q files(s) for mapping. These may be gzipped or not.

**\--longreads** *PATH ..*

  Longread FASTA/Q files(s) for mapping. These may be gzipped or not.

**-b**, **\--bam-files** *PATH*

  Path to BAM file(s). These must be reference sorted (e.g. with
    samtools sort) unless `--sharded` is specified, in which case they
    must be read name sorted (e.g. with `samtools sort -n`). When
    specified, no read mapping algorithm is undertaken.

**-l**, **\--longread-bam-files** *PATH*

  Path to longread BAM file(s). These must be reference sorted (e.g.
    with samtools sort) unless `--sharded` is specified, in which case
    they must be read name sorted (e.g. with `samtools sort -n`). When
    specified, no read mapping algorithm is undertaken.

# SHARDING

**\--sharded**

  If `-b/--bam-files` was used: Input BAM files are read-sorted
    alignments of a set of reads mapped to multiple reference contig
    sets. Choose the best hit for each read pair. Otherwise if mapping
    was carried out: Map reads to each reference, choosing the best hit
    for each pair. [default: not set]

# MAPPING ALGORITHM OPTIONS

**\--mapper** *NAME*

  Underlying mapping software used for short reads [default:
    `minimap2-sr`]. One of:

|                                             |                                                                                   |
|:--------------------------------------------|:----------------------------------------------------------------------------------|
| name                                        | \|description                                                                     |
| \-\-\-\-\-\-\-\-\-\-\-\-\-\-\-\-\-\-\-\-\-- | \|\-\-\-\-\-\-\-\-\-\-\-\-\-\-\-\-\-\-\-\-\-\-\-\-\-\-\-\-\-\-\-\-\-\-\-\-\-\-\-- |
| `minimap2-sr`                               | \|minimap2 with \'`-x sr`\' option                                                |
| `bwa-mem`                                   | \|bwa mem using default parameters                                                |
| `bwa-mem2`                                  | \|bwa-mem2 using default parameters                                               |
| `minimap2-ont`                              | \|minimap2 with \'`-x map-ont`\' option                                           |
| `minimap2-pb`                               | \|minimap2 with \'`-x map-pb`\' option                                            |
| `minimap2-hifi`                             | \|minimap2 with \'`-x map-hifi`\' option                                          |
| `minimap2-no-preset`                        | \|minimap2 with no \'`-x`\' option                                                |

**\--longread-mapper** *NAME*

  Underlying mapping software used for long reads [default:
    `minimap2-ont`]. One of:

|                                             |                                                                                   |
|:--------------------------------------------|:----------------------------------------------------------------------------------|
| name                                        | \|description                                                                     |
| \-\-\-\-\-\-\-\-\-\-\-\-\-\-\-\-\-\-\-\-\-- | \|\-\-\-\-\-\-\-\-\-\-\-\-\-\-\-\-\-\-\-\-\-\-\-\-\-\-\-\-\-\-\-\-\-\-\-\-\-\-\-- |
| `minimap2-ont`                              | \|minimap2 with \'`-x map-ont`\' option                                           |
| `minimap2-pb`                               | \|minimap2 with \'`-x map-pb`\' option                                            |
| `minimap2-hifi`                             | \|minimap2 with \'`-x map-hifi`\' option                                          |
| `minimap2-no-preset`                        | \|minimap2 with no \'`-x`\' option                                                |

**\--minimap2-params** *PARAMS*

  Extra parameters to provide to minimap2, both indexing command (if
    used) and for mapping. Note that usage of this parameter has
    security implications if untrusted input is specified. \'`-a`\' is
    always specified to minimap2. [default: none]

**\--minimap2-reference-is-index**

  Treat reference as a minimap2 database, not as a FASTA file.
    [default: not set]

**\--bwa-params** *PARAMS*

  Extra parameters to provide to BWA or BWA-MEM2. Note that usage of
    this parameter has security implications if untrusted input is
    specified. [default: none]

# ALIGNMENT THRESHOLDING

**\--min-read-aligned-length** *INT*

  Exclude reads with smaller numbers of aligned bases. [default:
    `0`]

**\--min-read-percent-identity** *FLOAT*

  Exclude reads by overall percent identity e.g. 95 for 95%.
    [default: `0`]

**\--min-read-aligned-percent** *FLOAT*

  Exclude reads by percent aligned bases e.g. 95 means 95% of the
    read\'s bases must be aligned. [default: `0`]

**\--min-read-aligned-length-pair** *INT*

  Exclude pairs with smaller numbers of aligned bases. Conflicts with
    \--allow-improper-pairs. [default: `0`]

**\--min-read-percent-identity-pair** *FLOAT*

  Exclude pairs by overall percent identity e.g. 95 for 95%. Conflicts
    with \--allow-improper-pairs. [default: `0`]

**\--min-read-aligned-percent-pair** *FLOAT*

  Exclude reads by percent aligned bases e.g. 95 means 95% of the
    read\'s bases must be aligned. Conflicts with
    \--allow-improper-pairs. [default: `0`]

**\--allow-improper-pairs**

  Require reads to be mapped as proper pairs. [default: not set]

**\--exclude-supplementary**

  Exclude supplementary alignments. [default: not set]

**\--include-secondary**

  Include secondary alignments. [default: not set]

**\--contig-end-exclusion** *INT*

  Exclude bases at the ends of reference sequences from calculation
    [default: 0]

**\--trim-min** *FLOAT*

  Remove this smallest fraction of positions when calculating
    trimmed_mean [default: 0.00]

**\--trim-max** *FLOAT*

  Maximum fraction for trimmed_mean calculations [default: 1.00]

**\--split-bams**

  Split the mapped read files up per reference. Useful if you think
    run time is being hampered by I/O. Most of the time this will not
    improve performance and instead just increase disk usage.

# VARIANT CALLING OPTIONS (BASIC)

**\--profile** *STR*

  Assembly profile to use for variant calling. Overrides
    \--kmer-sizes, \--min-prune-factor,
    \--allow-non-unique-kmers-in-ref, and
    \--recover-all-dangling-branches. Possible options include:
    \'fast\', \'very-fast\', \'sensitive\', \'precise\',
    \'super-sensitive\' [default: not_set]

**-k**, **\--kmer-sizes** *INT ..*

  K-mer sizes used to generate DeBruijn Graphs. Multiple values at
    once are accepted and encouraged at a cost to increased runtime e.g.
    17 21 25 [default: 21 33]

**\--ploidy** *INT*

  Sets the default ploidy for the analysis to N. [default: 2]

**\--calculate-fst**

  Calculate Fst values between samples and variants.

**\--calculate-dnds**

  Calculate coding regions and perform dN/dS calculations along them
    using called variants. \*Microbial only\*.

**-f**, **\--features-vcf** *PATH*

  The set of alleles to force-call regardless of evidence. Note: The
    sight containing these alleles has to be called as \'active\' in
    order for them to appear in the final VCF. Addtionally, Provided
    file must be compressed using bgzip and indexed using bcftools
    index. If no index is present, and index will be attempted to be
    created. If the file is not properly compressed, Lorikeet will
    unfortunately SEGFAULT with no error message.

**\--qual-by-depth-filter** *INT*

  The minimum QD value for a variant to have for it to be included in
    the genotyping or ANI analyses. [default: 25]

**\--qual-threshold** *INT*

  The PHRED-scaled quality score threshold for use with ANI
    calculations. [default: 150]

**\--depth-per-sample-filter** *INT*

  Minimum depth of a variant in a sample for that sample to be
    included in ANI & Fst calculations for that variant. [default: 5]

**\--min-long-read-size** *INT*

  The minimum size for long reads to be used for analysis [default:
    1500]

**\--min-long-read-average-base-qual** *INT*

  The minimum average base quality of a long read for it to be used
    for analysis [default: 20]

**-q**, **\--min-base-quality** *INT*

  Minimum base quality required to consider a base for calling.
    [default: 10]

**\--min-mapq** *INT*

  Minimum MAPQ score for reads to be considered during variant
    calling. [default: 20]

**\--base-quality-score-threshold** *INT*

  Base qualities below this threshold will be reduced to the minimum
    (6). [default: 18]

**\--max-input-depth** *INT*

  The maximum number of reads included within an assembly region
    across all samples. Larger numbers increase run time. If the depth
    of an assembly region exceeds this value, then the reads will be
    filtered by mean base quality. [default: 200000]

**\--min-contig-size** *INT*

  The minimum contig size to call variants on. Smaller contigs can
    often contain highly variable regions that mostly represent noise.
    Call variants on them can often be slow and not produce anything
    fruitful. [default: 0]

**\--min-sv-qual** *INT*

  Minimum structural variants quality returned by svim and used by
    lorikeet. Not PHRED-scaled quality, value determined by number of
    supporting reads. Consult svim documentation for details. [default:
    3]

**\--do-not-call-svs**

  Opts not to use svim to call structural variants using provided
    longreads. If no longreads are provided this has no effect.

# VARIANT CALLING OPTIONS (ADVANCED)

**\--phred-scaled-global-read-mismapping-rate** *INT*

  The global assumed mismapping rate for reads. [default: 45]

**\--pair-hmm-gap-continuation-penalty** *INT*

  Flat gap continuation penalty for use in the Pair HMM. [default:
    10]

**\--pcr-indel-model** *STR*

  The PCR indel model to use. [default: conservative]

**\--heterozygosity** *FLOAT*

  Heterozygosity value used to compute prior likelihoods for any
    locus. [default: 0.001]

**\--heterozygosity-stdev** *FLOAT*

  Standard deviation of heterozygosity for SNP and indel calling.
    [default: 0.01]

**\--indel-heterozygosity** *FLOAT*

  Heterozygosity for indel calling. [default: 0.000125]

**-C**, **\--standard-min-confidence-threshold-for-calling** *FLOAT*

  The minimum phred-scaled confidence threshold at which variants
    should be called. [default: 25.0]

**\--use-posteriors-to-calculate-qual**

  if available, use the genotype posterior probabilities to calculate
    the site QUAL.

**\--annotate-with-num-discovered-alleles**

  If provided, we will annotate records with the number of alternate
    alleles that were discovered (but not necessarily genotyped) at a
    given site.

**\--active-probability-threshold** *FLOAT*

  Minimum probability for a locus to be considered active. [default:
    0.002]

**\--min-assembly-region-size** *INT*

  Minimum size of an assembly region. [default: 50]

**\--max-assembly-region-size** *INT*

  Maximum size of an assembly region. [default: 300]

**\--assembly-region-padding** *INT*

  Number of additional bases of context to include around each
    assembly region. [default: 100]

**\--dont-increase-kmer-sizes-for-cycles**

  Disable iterating over kmer sizes when graph cycles are detected.

**\--allow-non-unique-kmers-in-ref**

  Allow graphs that have non-unique kmers in the reference. Can
    increase runtime and sensitivity in repeat regions.

**\--do-not-run-physical-phasing**

  Disable physical phasing.

**\--recover-all-dangling-branches**

  Recover all dangling branches.

**\--min-dangling-branch-length** *INT*

  Minimum length of a dangling branch to attempt recovery. [default:
    1]

**\--min-prune-factor** *INT*

  Minimum read support to not prune paths in the graph. Lower values
    like 1 or 0 will drastically increase runtime, but recover more
    variants. For faster performance, set this to 2 or 3 at a cost to
    recall. [default: 1]

**\--use-adaptive-pruning**

  Use more advanced pruning algorithm to prune paths in graph. Better
    suited when performing variant calling on when depth along a genome
    is variable e.g. RNA and exome data.

**\--num-pruning-samples** *INT*

  Number of samples that must pass the min_pruning threshold
    [default: 1]

**\--graph-output** *PATH*

  Write debug assembly graph information to this file.

**\--dont-use-soft-clipped-bases**

  Do not analyse soft clipped bases in the reads.

**\--initial-error-rate-for-pruning** *FLOAT*

  Initial base error rate estimate for adaptive pruning. [default:
    0.001]

**\--pruning-log-odds-threshold** *FLOAT*

  Likelihood ratio threshold for adaptive pruning algorithm. This
    value will be converted to log odds value. [default: 1.0]

**\--max-unpruned-variants** *INT*

  Maximum number of variants in graph the adaptive pruner will allow.
    [default: 100]

**\--max-prob-propagation-distance** *INT*

  Upper limit on how many bases away probability mass can be moved
    around when calculating the boundaries between active and inactive
    assembly regions. [default: 50]

**\--max-mnp-distance** *INT*

  Two or more phased substitutions separated by this distance or less
    are merged into MNPs. [default: 0]

**\--disable-optimizations**

  Don\'t skip calculations in ActiveRegions with no variants

**\--disable-avx**

  Disable the use of the GKL-rs AVX acceleration components for
    PairHMM and Smith-Waterman calculations.

**\--limiting-interval** *STR*

  Mainly used for debugging purposes. Only call variants within this
    given span on all contigs. E.g. providing \'1000-2000\' would only
    call variants between the 1000 and 2000 bp span on each provided
    contig.

**\--force**

  Forcefully overwrite previous runs.

# OUTPUT OPTIONS

**-o**, **\--output-directory** *DIRECTORY*

  Output directory. Folder will contain subfolders for each input
    genome [default: ./]

**\--bam-file-cache-directory** *DIRECTORY*

  Output BAM files generated during alignment to this directory. The
    directory may or may not exist. Note that BAM files in this
    directory contain all mappings, including those that later are
    excluded by alignment thresholding (e.g.
    \--min-read-percent-identity) or genome-wise thresholding (e.g.
    \--min-covered-fraction). [default: not used]

**\--keep-unmapped**

  Include unmapped reads from cached BAM files. [default: not set]

# FREQUENTLY ASKED QUESTIONS (FAQ)

**Can the temporary directory used be changed?** Lorikeet makes use of
the system temporary directory (often `/tmp`) to store intermediate
files. This can cause problems if the amount of storage available there
is small or used by many programs. To fix, set the `TMPDIR` environment
variable e.g. to set it to use the current directory:
`TMPDIR=. lorikeet call <etc>`

# EXIT STATUS

**0**

  Successful program execution.

**1**

  Unsuccessful program execution.

**101**

  The program panicked.

# EXAMPLES

Map paired reads to a reference and generate genotypes

  **\$ lorikeet genotype \--coupled read1.fastq.gz read2.fastq.gz
    \--reference assembly.fna \--threads 10 \--kmer-sizes 17 25 51**

Generate strain-level genotypes from read mappings compared to reference from a sorted BAM file and plots the results

  **\$ lorikeet genotype \--bam-files my.bam \--longread-bam-files
    my-longread.bam \--genome-fasta-directory genomes/ -x fna
    \--bam-file-cache-directory saved_bam_files \--output-directory
    lorikeet_out/ \--threads 10**

# AUTHOR

>     Rhys J. P. Newell, Centre for Microbiome Research, School of Biomedical Sciences, Faculty of Health, Queensland University of Technology <rhys.newell94 near gmail.com>
