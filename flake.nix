{
  description = "canvas-speed";

  inputs = {
    nixpkgs.url = "github:NixOS/nixpkgs/nixos-25.11";
    fenix = {
      url = "github:nix-community/fenix";
      inputs.nixpkgs.follows = "nixpkgs";
    };
    crane.url = "github:ipetkov/crane";
  };

  outputs =
    {
      self,
      nixpkgs,
      fenix,
      crane,
    }:
    let
      supportedSystems = [
        "x86_64-linux"
        "aarch64-linux"
        "x86_64-darwin"
        "aarch64-darwin"
      ];
      forEachSystem = nixpkgs.lib.genAttrs supportedSystems;
    in
    {
      perSystem = forEachSystem (
        system:
        let
          pkgs = nixpkgs.legacyPackages.${system};
          rustPkgs = fenix.packages.${system};

          rustToolchain = rustPkgs.combine (
            with rustPkgs.stable;
            [
              rust-analyzer
              clippy
              rustc
              cargo
              rustfmt
              rust-src
              rustPkgs.targets.x86_64-unknown-linux-musl.stable.rust-std
            ]
          );

          craneLib = (crane.mkLib pkgs).overrideToolchain rustToolchain;

          packages.default = craneLib.buildPackage {
            src = craneLib.cleanCargoSource self;
          };

          packages.static =
            let
              targetPkgs = pkgs.pkgsCross.musl64.pkgsStatic;
              targetCc = "${targetPkgs.stdenv.cc}/bin/${targetPkgs.stdenv.cc.targetPrefix}cc";
            in
            craneLib.buildPackage {
              src = craneLib.cleanCargoSource self;
              TARGET_CC = targetCc;
              CARGO_BUILD_TARGET = "x86_64-unknown-linux-musl";
              CARGO_BUILD_RUSTFLAGS = [
                "-C"
                "target-feature=+crt-static"

                "-C"
                "link-args=-static"

                # https://github.com/rust-lang/cargo/issues/4133
                "-C"
                "linker=${targetCc}"
              ];
            };
        in
        {
          devShells.default = pkgs.mkShell rec {
            nativeBuildInputs = [
              rustToolchain
              pkgs.cargo-deny
              pkgs.cargo-edit
              pkgs.cargo-watch
            ];

            buildInputs = [
              pkgs.pkg-config
              pkgs.openssl
            ];

            env = {
              RUST_SRC_PATH = "${rustToolchain}/lib/rustlib/src/rust/library";
            };

            LD_LIBRARY_PATH = "${pkgs.lib.makeLibraryPath buildInputs}";

          };

          inherit packages;
        }
      );

      devShells = forEachSystem (system: self.perSystem.${system}.devShells);
      packages = forEachSystem (system: self.perSystem.${system}.packages);
      formatter = forEachSystem (system: nixpkgs.legacyPackages.${system}.nixfmt-tree);
    };
}
