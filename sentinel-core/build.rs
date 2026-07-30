fn main() {
    let disabled = "surface_matching.hpp,surface_matching/ppf_helpers.hpp,surface_matching/t_position_hash_table.hpp,opencv2/surface_matching.hpp,opencv2/surface_matching/ppf_helpers.hpp";
    std::env::set_var("OPENCV_DISABLE_HEADERS", disabled);
    println!("cargo:rustc-env=OPENCV_DISABLE_HEADERS={}", disabled);
}
