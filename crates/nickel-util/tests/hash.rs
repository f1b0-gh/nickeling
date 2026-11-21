use nickel_util::files;

#[test]
fn sha256_hashes_keep_their_lowercase_zero_padded_representation() {
    assert_eq!(
        files::hash(b""),
        "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855"
    );
    assert_eq!(
        files::hash(b"abc"),
        "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
    );
}
