#!/usr/bin/env python3

import subprocess
import logging
import argparse
import os


def remove_before(marker, string_to_process):
    splitter = '\n# ' + marker + '\n'
    if splitter not in string_to_process:
        import IPython; IPython.embed()
        raise Exception("Marker '{}' not found in string".format(marker))
    return splitter + string_to_process.split(splitter)[1]


def get_version():
    output = subprocess.check_output(['bash','-c','cargo run -- --version']).strip()
    return output.decode('utf-8').split(' ')[1]


if __name__ == '__main__':
    parent_parser = argparse.ArgumentParser(add_help=False)
    # parent_parser.add_argument('--debug', help='output debug information', action="store_true")
    # parent_parser.add_argument('--version', help='output version information and quit',  action='version', version=repeatm.__version__)
    parent_parser.add_argument('--quiet', help='only output errors', action="store_true")

    args = parent_parser.parse_args()

    # Setup logging
    debug = True
    if args.quiet:
        loglevel = logging.ERROR
    else:
        loglevel = logging.DEBUG
    logging.basicConfig(level=loglevel, format='%(asctime)s %(levelname)s: %(message)s', datefmt='%m/%d/%Y %I:%M:%S %p')

    # Update [RELEASE_TAG] in installation.md
    version = get_version()
    logging.info("Updating [RELEASE_TAG] in Installation.md to {}".format(version))
    with open('docs/Installation.md.in') as f:
        installation = f.read()
    installation = installation.replace('[RELEASE_TAG]', version)
    with open('docs/Installation.md', 'w') as f:
        f.write(installation)
    logging.info("Done updating [RELEASE_TAG] in Installation.md to {}".format(version))

    subdir_and_commands = [
        ['tools', ['genotype','call','summarise','consensus']]
    ]

    for subdir, commands in subdir_and_commands:
        for subcommand in commands:
            cmd_stub = "cargo run -- {} --full-help-roff |pandoc - -t markdown-multiline_tables-simple_tables-grid_tables -f man |sed 's/\\\\\\[/[/g; s/\\\\\\]/]/g; s/^: //'".format(subcommand)
            man_usage = subprocess.check_output(['bash','-c',cmd_stub]).decode("utf-8") 

            subcommand_prelude = 'docs/preludes/{}_prelude.md'.format(subcommand)
            if os.path.exists(subcommand_prelude):
                # Remove everything before the options section
                splitters = {
                    # 'pipe': 'COMMON OPTIONS',
                    # 'microbial_fraction': 'OPTIONS',
                    # 'data': 'OPTIONS',
                    # 'summarise': 'TAXONOMIC PROFILE INPUT',
                    # 'makedb': 'REQUIRED ARGUMENTS',
                    # 'appraise': 'INPUT OTU TABLE OPTIONS',
                    # 'seqs': 'OPTIONS',
                    # 'metapackage': 'OPTIONS',
                }
                logging.info("For ROFF for command {}, removing everything before '{}'".format(
                    subcommand, splitters[subcommand]))
                man_usage = remove_before(splitters[subcommand], man_usage)

                with open('docs/{}/{}.md'.format(subdir, subcommand),'w') as f:
                    f.write('---\n')
                    f.write('title: Lorikeet {}\n'.format(subcommand))
                    f.write('---\n')
                    f.write('# lorikeet {}\n'.format(subcommand))

                    with open(subcommand_prelude) as f2:
                        f.write(f2.read())

                    f.write(man_usage)
            else:
                man_usage = remove_before('DESCRIPTION', man_usage)
                with open('docs/{}/{}.md'.format(subdir, subcommand),'w') as f:
                    f.write('---\n')
                    f.write('title: Lorikeet {}\n'.format(subcommand))
                    f.write('---\n')
                    f.write('# lorikeet {}\n'.format(subcommand))

                    f.write(man_usage)

    subprocess.check_output(['bash','-c',"doctave build"])
