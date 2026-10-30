//! Print the sources that the models of the catalogs need, as JSON, for
//! `scripts/model-descriptors.py` (see `sayso_core::engine_catalog::descriptors`).

fn main() {
    let wanted = sayso_core::engine_catalog::descriptors::wanted();
    println!("{}", serde_json::to_string_pretty(&wanted).expect("the sources are JSON"));
}
