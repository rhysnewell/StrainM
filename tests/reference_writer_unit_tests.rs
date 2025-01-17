#![allow(
    non_upper_case_globals,
    non_snake_case
)]

use lorikeet_genome::model::byte_array_allele::ByteArrayAllele;
use lorikeet_genome::model::variant_context::{VariantContext, VariantType};
use lorikeet_genome::reference::reference_writer::ReferenceWriter;

#[test]
fn test_indel_offsetting() {
    let mut bases = vec![b'A'; 100];
    let mut offset = 0;
    bases.splice(3.., vec![b'G'; 97].into_iter());
    let original_bases = bases.clone();

    let snp_allele = ByteArrayAllele::new(b"T", false);
    let ref_allele = ByteArrayAllele::new(b"A", true);
    let mut snp_vc = VariantContext::build(0, 0, 0, vec![ref_allele.clone(), snp_allele.clone()]);

    let mut expected_bases = bases.clone();
    expected_bases[0] = b'T';

    ReferenceWriter::modify_reference_bases_based_on_variant_type(
        &mut bases,
        snp_allele,
        &mut snp_vc,
        VariantType::Snp,
        &mut offset,
    );

    assert_eq!(offset, 0);
    assert_eq!(
        &expected_bases,
        &bases,
        "\n Expected:\n {:?} \n Actual:\n {:?}",
        std::str::from_utf8(&expected_bases).unwrap(),
        std::str::from_utf8(&bases).unwrap()
    );

    let insertion_allele = ByteArrayAllele::new(b"ACCCCCC", false);
    let mut insertion_vc =
        VariantContext::build(0, 1, 1, vec![ref_allele, insertion_allele.clone()]);

    expected_bases.splice(2..2, vec![b'C'; 6].into_iter());

    ReferenceWriter::modify_reference_bases_based_on_variant_type(
        &mut bases,
        insertion_allele,
        &mut insertion_vc,
        VariantType::Indel,
        &mut offset,
    );

    assert_eq!(offset, 6);
    assert_eq!(
        &expected_bases,
        &bases,
        "\n Expected:\n {:?} \n Actual:\n {:?}",
        std::str::from_utf8(&expected_bases).unwrap(),
        std::str::from_utf8(&bases).unwrap()
    );
    assert_eq!(
        bases.len(),
        106,
        "\n Expected:\n {:?} \n Actual:\n {:?}",
        std::str::from_utf8(&expected_bases).unwrap(),
        std::str::from_utf8(&bases).unwrap()
    );

    let deletion_allele = ByteArrayAllele::new(b"A", false);
    let ref_allele = ByteArrayAllele::new(b"AAAAAA", true);
    let mut deletion_vc =
        VariantContext::build(0, 2, 7, vec![ref_allele, deletion_allele.clone()]);

    expected_bases.splice(9..=13, vec![b'A'; 1].into_iter().skip(1));

    ReferenceWriter::modify_reference_bases_based_on_variant_type(
        &mut bases,
        deletion_allele,
        &mut deletion_vc,
        VariantType::Indel,
        &mut offset,
    );

    assert_eq!(offset, 1);
    assert_eq!(
        &expected_bases,
        &bases,
        "\n Expected:\n {:?} \n Actual:\n {:?}",
        std::str::from_utf8(&expected_bases).unwrap(),
        std::str::from_utf8(&bases).unwrap()
    );
    assert_eq!(
        bases.len(),
        101,
        "\n Expected:\n {:?} \n Actual:\n {:?}",
        std::str::from_utf8(&expected_bases).unwrap(),
        std::str::from_utf8(&bases).unwrap()
    );

    println!("{}", std::str::from_utf8(&original_bases).unwrap());
    println!("{}", std::str::from_utf8(&expected_bases).unwrap());
    println!("{}", std::str::from_utf8(&bases).unwrap());
}

// TTTTTCGGTAATAAAATGATGATCGTTATTTGTATCTAACGACCCGTTA
// TTTTTCGGTAATAAAATGATGACCCCCCCTCCGTATCTAACGACCCGTTA
