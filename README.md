![](https://travis-ci.com/rhysnewell/Lorikeet.svg?branch=master)
![](https://anaconda.org/bioconda/lorikeet-genome/badges/license.svg)
![](https://anaconda.org/bioconda/lorikeet-genome/badges/version.svg)
![](https://anaconda.org/bioconda/lorikeet-genome/badges/latest_release_relative_date.svg)
![](https://anaconda.org/bioconda/lorikeet-genome/badges/platforms.svg)


![](docs/_include/images/lorikeet_logo.png)

## Lorikeet

Lorikeet is a within-species variant analysis pipeline for metagenomic communities that utilizes both long and short reads.
Lorikeet utilizes a re-implementaion of the GATK HaplotypeCaller algorithm, performing local re-assembly of potentially active
regions within candidate genomes. Called variants can be clustered into likely strains using a combination of UMAP and HDBSCAN.

## Documentation

For detailed documentation of Lorikeet and the various algorithms and concepts it touches on please visit the 
[Lorikeet Docs](https://rhysnewell.github.io/Lorikeet)


## Quick Start

### Installation

#### Option 1: Build manually
You may need to manually set the paths for `C_INCLUDE_PATH`, `LIBRARY_PATH`, `LIBCLANG_PATH`, and `OPENSSL_DIR` to their corresponding
paths in the your conda environment if they can't properly be found on your system. This method also assumes you have 
previously installed rust via rustup on your system.
```
GIT_LFS_SKIP_SMUDGE=1 git clone --recursive https://github.com/rhysnewell/Lorikeet.git;
cd Lorikeet;
conda env create -n lorikeet -f lorikeet.yml; 
conda activate lorikeet;
pip install --upgrade cmake;
bash install.sh # or run without installing e.g. `cargo run --release -- call -h`;
lorikeet call -h
```

Depending on your local network configuration, you may have problems obtaining Lorikeet via git.
If you see something like this you may be behind a proxy that blocks access to standard git:// port (9418).

```
$ git clone --recursive git://github.com/rhysnewell/Lorikeet.git
Cloning into 'Lorikeet'...
fatal: Unable to look up github.com (port 9418) (Name or service not known)
```

Luckily, thanks to this handy tip from the developer of [Freebayes](https://github.com/ekg/freebayes) we can work around it.
If you have access to https:// on port 443, then you can use this 'magic' command as a workaround to enable download of the submodules:

```
git config --global url.https://github.com/.insteadOf git://github.com/
```

#### Option 2: Conda 

Install into current conda environment:
```
conda install -c conda-forge bioconda lorikeet-genome
```

Create fresh conda environment and install lorikeet there:
```
conda create -n lorikeet -c conda-forge bioconda lorikeet-genome && \
conda activate lorikeet
```

#### Option 3: Install static binary **Not currently recommended**
The static binary is the easiest to use, however it is compiled with `musl` which for some reason makes Lorikeet (and other
rust binaries) perform much slower than they usually should. As such, we do not recommend using this static binary until
this problem is sorted out.

You can make use of the precompiled static binaries that come with this repository. You will have to install the lorikeet
conda environment using the lorikeet.yml.
```
GIT_LFS_SKIP_SMUDGE=1 git clone --recursive https://github.com/rhysnewell/Lorikeet.git;
cd Lorikeet;
conda env create -n lorikeet -f lorikeet.yml;
conda activate lorikeet
```

Once you have created the conda environment download and install the latest release file from github
```
wget https://github.com/rhysnewell/Lorikeet/releases/download/latest/lorikeet-x86_64-unknown-linux-musl-v0.6.1.tar.gz;
tar -xvzf lorikeet-x86_64-unknown-linux-musl-v*.tar.gz;
cp release/lorikeet $CONDA_PREFIX/bin;
cp release/remove_minimap2_duplicated_headers $CONDA_PREFIX/bin;
```

## Usage

Input can either be reads and reference genome, or MAG. Or a BAM file and associated genome.

```
Strain genotyping analysis for metagenomics

Usage: lorikeet <subcommand> ...

Main subcommands:
    genotype    *Experimental* Resolve strain-level genotypes of MAGs from microbial communities
    consensus   Creates consensus genomes for each input reference and for each sample
    call        Performs variant calling with no downstream analysis
    evolve      Calculate dN/dS values for genes from read mappings

Other options:
    -V, --version   Print version information

Rhys J. P. Newell <r.newell near uq.edu.au>
```

Call variants from bam:

`lorikeet call --bam-files my.bam --longread-bam-files my-longread.bam --genome-fasta-directory genomes/ -x fna
     --bam-file-cache-directory saved_bam_files --output-directory lorikeet_out/ --threads 10 --plot`

Call variants from short reads and longread bam files:

`lorikeet call -r input_genome.fna -1 forward_reads.fastq -2 reverse_reads.fastq -l longread.bam`


## Shell completion

Completion scripts for various shells e.g. BASH can be generated. For example, to install the bash completion script system-wide (this requires root privileges):

```
lorikeet shell-completion --output-file lorikeet --shell bash
mv lorikeet /etc/bash_completion.d/
```

It can also be installed into a user's home directory (root privileges not required):

```
lorikeet shell-completion --shell bash --output-file /dev/stdout >>~/.bash_completion
```

In both cases, to take effect, the terminal will likely need to be restarted. To test, type `lorikeet ca` and it should complete after pressing the TAB key.

## License

Code is [GPL-3.0](LICENSE)
