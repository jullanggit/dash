{
  description = "Dashboard Devshell";

  inputs = {
    host.url = "git+file:///etc/nixos";

    nixpkgs.follows = "host/nixpkgs";

    rust-overlay = {
      url = "github:oxalica/rust-overlay";
      inputs.nixpkgs.follows = "nixpkgs";
    };

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
    flake-utils.lib.eachDefaultSystem (
      system:
      let
        pkgs = import nixpkgs {
          inherit system;
          overlays = [ rust-overlay.overlays.default ];
        };

        rustToolchain = pkgs.rust-bin.selectLatestNightlyWith (
          toolchain:
          toolchain.default.override {
            targets = [ "wasm32-unknown-unknown" ];
            extensions = [
              "rust-src"
              "rust-analyzer"
            ];
          }
        );
      in
      {
        devShells.default =
          let
            jemalloc-tikv = pkgs.jemalloc.overrideAttrs (oldAttrs: {
              configureFlags = (oldAttrs.configureFlags or [ ]) ++ [
                "--with-jemalloc-prefix=_rjem_"
                "--with-private-namespace=_rjem_"
              ];
            });
          in
          pkgs.mkShell {
            packages = with pkgs; [
              rustToolchain
              dioxus-cli
              vtsls
              eslint
              swc
              just
              tombi
              bacon
              tailwindcss
              openssl
              pkg-config
              binaryen # wasm-opt
              jemalloc-tikv
              podman
            ];
            JEMALLOC_OVERRIDE = "${jemalloc-tikv}/lib/libjemalloc.a";
          };
      }
    );
}
