//! With the `parakeet` feature on Linux, build parakeet.cpp with ggml's Vulkan backend and link it
//! statically. The build needs CMake, the Vulkan headers and `glslc` (`scripts/linux-deps.sh`, and on Ubuntu
//! 22.04 `scripts/linux-vulkan-deps.sh`); the engine then needs only `libvulkan.so.1` at run time.
//!
//! The sources are two pinned GitHub archives, checked by SHA-256: parakeet.cpp, and the ggml commit that
//! is its submodule (an archive leaves submodules out). `SAYSO_PARAKEET_CPP_DIR` builds a local parakeet.cpp
//! tree instead, with ggml in `third_party/ggml` and its patches applied, and downloads nothing.

fn main() {
    println!("cargo:rerun-if-env-changed=SAYSO_PARAKEET_CPP_DIR");
    // The build dependencies are for a Linux host only (Cargo.toml), so this part compiles only there.
    #[cfg(target_os = "linux")]
    if std::env::var_os("CARGO_FEATURE_PARAKEET").is_some() && std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("linux") {
        parakeet_cpp::build();
    }
}

#[cfg(target_os = "linux")]
mod parakeet_cpp {
    use sha2::{Digest, Sha256};
    use std::path::{Path, PathBuf};

    struct Archive {
        url: &'static str,
        sha256: &'static str,
    }

    const PARAKEET_CPP: Archive = Archive {
        url: "https://github.com/mudler/parakeet.cpp/archive/5c9b0fbbd8e611d88e47d7e1ebae9bca9b447e8f.tar.gz",
        sha256: "a6544898a6721a9ab54a8af37362cb9a79a8b50feaf16a597091790a8874debe",
    };

    /// parakeet.cpp's `third_party/ggml` submodule at that commit. A new pin also updates the license copies
    /// in `licenses/`, which the packages ship (scripts/bundle-linux.sh).
    const GGML: Archive = Archive {
        url: "https://github.com/ggml-org/ggml/archive/e705c5fed490514458bdd2eaddc43bd098fcce9b.tar.gz",
        sha256: "77eacb7cc20c2e47ffbaa5a69fe6f9fd9da0bea2a6431b7fae1a8283cb2c4e43",
    };

    pub fn build() {
        let source = match std::env::var_os("SAYSO_PARAKEET_CPP_DIR") {
            Some(dir) => {
                let dir = PathBuf::from(dir);
                for part in ["src", "include", "third_party/ggml/src"] {
                    println!("cargo:rerun-if-changed={}", dir.join(part).display());
                }
                // A clone without `--recursive` has an empty third_party/ggml.
                assert!(
                    dir.join("third_party/ggml/CMakeLists.txt").exists(),
                    "{} has no ggml in third_party/ggml",
                    dir.display()
                );
                dir
            }
            None => fetch(),
        };
        let out = cmake::Config::new(&source)
            .define("BUILD_SHARED_LIBS", "OFF")
            .define("PARAKEET_SHARED", "OFF")
            .define("PARAKEET_GGML_VULKAN", "ON")
            .define("PARAKEET_WITH_CED", "OFF")
            .define("PARAKEET_WITH_VOICEDETECT", "OFF")
            .define("PARAKEET_BUILD_CLI", "OFF")
            .define("PARAKEET_BUILD_SERVER", "OFF")
            .define("PARAKEET_BUILD_TESTS", "OFF")
            .define("GGML_NATIVE", "OFF") // the release binaries run on other CPUs
            .define("GGML_OPENMP", "OFF") // else the engine needs libgomp at run time
            .build_target("parakeet")
            .build();
        let build = out.join("build");
        for dir in ["", "third_party/ggml/src", "third_party/ggml/src/ggml-cpu", "third_party/ggml/src/ggml-vulkan"] {
            println!("cargo:rustc-link-search=native={}", build.join(dir).display());
        }
        for lib in ["parakeet", "ggml", "ggml-cpu", "ggml-vulkan", "ggml-base"] {
            println!("cargo:rustc-link-lib=static={lib}");
        }
        println!("cargo:rustc-link-lib=dylib=vulkan");
        println!("cargo:rustc-link-lib=dylib=stdc++");
    }

    /// The parakeet.cpp tree in `OUT_DIR`, downloaded once. The marker file is written last, so a tree that a
    /// failed build or a cache left without its files is downloaded again.
    fn fetch() -> PathBuf {
        let root = PathBuf::from(std::env::var("OUT_DIR").unwrap()).join("parakeet.cpp");
        let marker = root.join(".complete");
        if std::fs::read_to_string(&marker).is_ok_and(|pins| pins == pins_text()) {
            return root;
        }
        if root.exists() {
            std::fs::remove_dir_all(&root).unwrap();
        }
        unpack(&PARAKEET_CPP, &root);
        let ggml = root.join("third_party/ggml");
        unpack(&GGML, &ggml);
        // parakeet.cpp's CMake applies these only to a git checkout, and only warns otherwise.
        let mut patches: Vec<_> = std::fs::read_dir(root.join("third_party/ggml-patches"))
            .unwrap()
            .map(|e| e.unwrap().path())
            .filter(|p| p.extension().is_some_and(|x| x == "patch"))
            .collect();
        patches.sort();
        for patch in patches {
            let status = std::process::Command::new("patch")
                .args(["-p1", "--forward", "--silent", "-d"])
                .arg(&ggml)
                .arg("-i")
                .arg(&patch)
                .status()
                .expect("the parakeet feature needs the `patch` program");
            assert!(status.success(), "cannot apply {}", patch.display());
        }
        std::fs::write(&marker, pins_text()).unwrap();
        root
    }

    fn pins_text() -> String {
        format!("{}\n{}\n", PARAKEET_CPP.sha256, GGML.sha256)
    }

    /// Download a `.tar.gz`, check its SHA-256, and unpack it into `dest` without its top folder.
    fn unpack(archive: &Archive, dest: &Path) {
        let mut bytes = Vec::new();
        std::io::Read::read_to_end(
            &mut ureq::get(archive.url)
                .call()
                .unwrap_or_else(|e| panic!("cannot download {}: {e}", archive.url))
                .into_body()
                .into_reader(),
            &mut bytes,
        )
        .unwrap_or_else(|e| panic!("cannot download {}: {e}", archive.url));
        let sha256 = format!("{:x}", Sha256::digest(&bytes));
        assert_eq!(sha256, archive.sha256, "{} has another SHA-256", archive.url);
        std::fs::create_dir_all(dest).unwrap();
        let mut tar = tar::Archive::new(flate2::read::GzDecoder::new(bytes.as_slice()));
        for entry in tar.entries().unwrap() {
            let mut entry = entry.unwrap();
            let path = entry.path().unwrap().components().skip(1).collect::<PathBuf>();
            if !path.as_os_str().is_empty() {
                let path = dest.join(path);
                std::fs::create_dir_all(path.parent().unwrap()).unwrap();
                entry.unpack(path).unwrap();
            }
        }
    }
}
