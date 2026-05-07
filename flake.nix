{
  description = "ADHD nudge timer for Wayland — flashes red overlay on interval, locks screen when done";

  inputs = {
    nixpkgs.url = "github:NixOS/nixpkgs/nixos-unstable";
    rust-overlay.url = "github:oxalica/rust-overlay";
    flake-utils.url = "github:numtide/flake-utils";
  };

  outputs =
    {
      self,
      nixpkgs,
      rust-overlay,
      flake-utils,
      ...
    }:
    {
      overlays.default = final: prev: {
        nudge = self.packages.${prev.system}.default;
      };
    }
    // flake-utils.lib.eachDefaultSystem (
      system:
      let
        overlays = [ (import rust-overlay) ];

        pkgs = import nixpkgs { inherit system overlays; };

        toolchain = pkgs.rust-bin.stable.latest.default.override { extensions = [ "rust-src" ]; };

        nightlyToolchain = pkgs.rust-bin.nightly.latest.default.override {
          extensions = [ "rust-src" ];
        };

        cargo-nightly = pkgs.writeShellScriptBin "cargo-nightly" ''
          export RUSTC_BOOTSTRAP=1
          export PATH="${nightlyToolchain}/bin:$PATH"
          cargo "$@"
        '';

        cargoToml = builtins.fromTOML (builtins.readFile ./Cargo.toml);

        cargo-extensions = with pkgs; [
          cargo-audit
          cargo-edit
          bacon
          cargo-udeps
          cargo-unused-features
        ];

        extra-tools = with pkgs; [
          typos
          cargo-nightly
        ];

        # iced + iced_layershell runtime deps. same set as launcher.
        libraries = with pkgs; [
          wayland
          libxkbcommon
          libGL
          glib
        ];

        format-all = pkgs.writeShellScriptBin "format-all" ''
          set -e
          echo "Formatting Nix files..."
          find . -type f -name "*.nix" -exec ${pkgs.nixfmt-rfc-style}/bin/nixfmt {} \;

          echo "Formatting Rust files..."
          export PATH="${nightlyToolchain}/bin:$PATH"
          cargo fmt
        '';
      in
      {
        formatter = format-all;

        devShells.default = pkgs.mkShell {
          nativeBuildInputs = with pkgs; [
            pkg-config
          ];

          buildInputs =
            with pkgs;
            [
              pkg-config
              toolchain
              gcc
              mold-wrapped
            ]
            ++ cargo-extensions
            ++ libraries
            ++ extra-tools;

          env = {
            LD_LIBRARY_PATH = "${nixpkgs.lib.makeLibraryPath libraries}";
            RUST_LOG = "debug";
          };

          shellHook = ''
            echo "Rust dev shell for nudge loaded."
            echo "Run: cargo run -- 10s -d 0.5 -f 2s   # quick e2e test"
          '';
        };

        checks = {
          format = pkgs.runCommand "check-format" { buildInputs = [ nightlyToolchain ]; } ''
            cd ${self}
            cargo fmt --check
            touch $out
          '';

          clippy = pkgs.runCommand "check-clippy" { buildInputs = [ toolchain ]; } ''
            cd ${self}
            cargo clippy -- -D warnings
            touch $out
          '';
        };

        packages = {
          default = pkgs.rustPlatform.buildRustPackage {
            pname = cargoToml.package.name;
            version = cargoToml.package.version;
            src = ./.;
            cargoLock = {
              lockFile = ./Cargo.lock;
              # add outputHashes here later if any git deps are pulled in
            };

            nativeBuildInputs = with pkgs; [
              pkg-config
              toolchain
              makeWrapper
            ];

            buildInputs = libraries;

            postFixup = ''
              wrapProgram $out/bin/nudge \
                --prefix LD_LIBRARY_PATH : "${nixpkgs.lib.makeLibraryPath libraries}"
            '';
          };
        };
      }
    );
}
