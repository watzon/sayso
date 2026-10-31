//! Run the migration of the model store on a copy of a models folder.
//!
//! ```text
//! cargo run -p sayso-core --example migrate_models -- /tmp/copy/models
//! ```

use std::path::PathBuf;

fn main() {
    let folder = PathBuf::from(std::env::args().nth(1).expect("give the folder of a copy"));
    let path = folder.canonicalize().expect("the folder exists");
    // The real folder is for Sayso only. This program is for a copy.
    assert!(!path.to_string_lossy().contains("Application Support/Sayso"), "this is the real models folder: {}", path.display());
    let begin = std::time::Instant::now();
    let report = sayso_core::model_migration::run(&path).expect("the migration");
    println!("took {:?}", begin.elapsed());
    for model in &report.moved {
        println!("moved {model}");
    }
    for (model, reason) in &report.kept {
        println!("kept {model}: {reason}");
    }
}
