---
title: Lorikeet summarise
---
# lorikeet summarise

# DESCRIPTION

lorikeet summarise uses a set of VCF files as input and calculates
conANI, popANI, subpopANI, and Fst metrics for the variants in each
file. ANI metrics require coverage information to determine the number
of shared bases in each sample. VCF files do not provide this
information, so the shared base size is just the total size of the
genome. In our experience, this doesn\'t really matter that much as the
ANI metrics are quite insensitive when provided low coverage samples
anyway. Fst tends to perform better for low and high coverage samples
and does not require whole genome coverage information.

# FLAGS

**-v**, **\--verbose**

  Print extra debugging information. [default: not set]

**-q**, **\--quiet**

  Unless there is an error, do not print log messages. [default: not
    set]

# OPTIONS

**-i**, **\--vcfs** *PATH ..*

  Paths to input VCF files. Can provide one or more.

**-i**, **\--vcfs** *DIRECTORY*

  Paths to input VCF files. Can provide one or more.

**-o**, **\--output-directory** *DIRECTORY*

  Output directory. Folder will contain subfolders for each input VCF
    [default: ./]

**-t**, **\--threads** *INT*

  Maximum number of threads used. [default: 8]

**\--qual-by-depth-filter** *INT*

  The minimum QD value for a variant to have for it to be included in
    the genotyping or ANI analyses. [default: 25]

**\--qual-threshold** *INT*

  The PHRED-scaled quality score threshold for use with ANI
    calculations. [default: 150]

**\--depth-per-sample-filter** *INT*

  Minimum depth of a variant in a sample for that sample to be
    included in ANI & Fst calculations for that variant. [default: 5]

# EXIT STATUS

**0**

  Successful program execution.

**1**

  Unsuccessful program execution.

**101**

  The program panicked.

# AUTHOR

>     Rhys J. P. Newell, Centre for Microbiome Research, School of Biomedical Sciences, Faculty of Health, Queensland University of Technology <rhys.newell94 near gmail.com>
