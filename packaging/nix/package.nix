# Sayso from the Linux tarball of a GitHub release. The binaries are not built
# here: a build from source needs the network (the sherpa-onnx library).
# release.json has the version and the hashes. scripts/nix-release.sh writes it.
{
  lib,
  stdenv,
  fetchurl,
  autoPatchelfHook,
  alsa-lib,
  fontconfig,
  freetype,
  libGL,
  libxcb,
  libxkbcommon,
  vulkan-loader,
  wayland,
}:
let
  release = lib.importJSON ./release.json;
  arch = stdenv.hostPlatform.parsed.cpu.name;
in
stdenv.mkDerivation {
  pname = "sayso";
  inherit (release) version;

  src = fetchurl {
    url = "https://github.com/watzon/sayso/releases/download/v${release.version}/sayso-${release.version}-linux-${arch}.tar.gz";
    hash = release.hashes.${arch} or (throw "Sayso has no release file for ${arch}");
  };

  nativeBuildInputs = [ autoPatchelfHook ];
  buildInputs = [
    alsa-lib
    fontconfig
    freetype
    libxcb
    libxkbcommon
    stdenv.cc.cc.lib
  ];
  # The app loads these at run time (Wayland, and Vulkan with a GL fallback).
  runtimeDependencies = [
    libGL
    vulkan-loader
    wayland
  ];
  dontStrip = true;

  # The app finds the engine next to itself, so both binaries live in one folder.
  installPhase = ''
    runHook preInstall
    install -Dm755 bin/sayso $out/lib/sayso/sayso
    install -Dm755 bin/sayso-engine $out/lib/sayso/sayso-engine
    mkdir -p $out/bin
    ln -s $out/lib/sayso/sayso $out/bin/sayso
    cp -r share $out/share
    install -Dm644 lib/udev/rules.d/70-sayso-uinput.rules $out/lib/udev/rules.d/70-sayso-uinput.rules
    runHook postInstall
  '';

  meta = {
    description = "Local voice dictation into any app";
    homepage = "https://justsayso.app";
    license = lib.licenses.gpl3Only;
    mainProgram = "sayso";
    platforms = [
      "x86_64-linux"
      "aarch64-linux"
    ];
    sourceProvenance = [ lib.sourceTypes.binaryNativeCode ];
  };
}
