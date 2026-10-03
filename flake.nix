{
  description = "Sayso, local voice dictation into any app";

  inputs.nixpkgs.url = "github:NixOS/nixpkgs/nixos-unstable";

  outputs =
    { self, nixpkgs }:
    let
      systems = [
        "x86_64-linux"
        "aarch64-linux"
      ];
      forEachSystem = f: nixpkgs.lib.genAttrs systems (system: f nixpkgs.legacyPackages.${system});
    in
    {
      packages = forEachSystem (pkgs: rec {
        sayso = pkgs.callPackage ./packaging/nix/package.nix { };
        default = sayso;
      });

      overlays.default = final: _prev: {
        sayso = final.callPackage ./packaging/nix/package.nix { };
      };

      # programs.sayso.enable installs Sayso and its udev rule for /dev/uinput
      # (the paste key on Wayland desktops that have no other way).
      nixosModules.default =
        {
          config,
          lib,
          pkgs,
          ...
        }:
        let
          cfg = config.programs.sayso;
        in
        {
          options.programs.sayso = {
            enable = lib.mkEnableOption "Sayso, local voice dictation";
            package = lib.mkOption {
              type = lib.types.package;
              default = self.packages.${pkgs.stdenv.hostPlatform.system}.sayso;
              description = "The Sayso package.";
            };
          };
          config = lib.mkIf cfg.enable {
            environment.systemPackages = [ cfg.package ];
            services.udev.packages = [ cfg.package ];
          };
        };
    };
}
