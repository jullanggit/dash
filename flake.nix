{
  description = "Dashboard Devshell";

  inputs = {
    nixpkgs.url = "github:nixos/nixpkgs";

    rust-overlay = {
      url = "github:oxalica/rust-overlay/860d7c835ab91bfc8972b67092f5f2db8e9390a0";
      inputs.nixpkgs.follows = "nixpkgs";
    };

    flake-utils.url = "github:numtide/flake-utils/11707dc2f618dd54ca8739b309ec4fc024de578b";
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
              # swc TODO: reenable once it stops failing to compile
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
            JEMALLOC_OVERRIDE = "${jemalloc-tikv}/lib/libjemalloc.so";
          };
      }
    );
}
