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
          config = {
            android_sdk.accept_license = true;
            allowUnfree = true;
          };
        };

        mkShell =
          type:
          pkgs.mkShell (
            let
              rustToolchain = pkgs.rust-bin.selectLatestNightlyWith (
                toolchain:
                toolchain.default.override {
                  targets = [
                    "wasm32-unknown-unknown"
                  ]
                  ++ (
                    if type == "android" then
                      [
                        "aarch64-linux-android"
                        "x86_64-linux-android"
                      ]
                    else
                      [ ]
                  );
                  extensions = [
                    "rust-src"
                    "rust-analyzer"
                  ];
                }
              );

              jemalloc-tikv = pkgs.jemalloc.overrideAttrs (oldAttrs: {
                configureFlags = (oldAttrs.configureFlags or [ ]) ++ [
                  "--with-jemalloc-prefix=_rjem_"
                  "--with-private-namespace=_rjem_"
                ];
              });

              # Android 17 (API 37)
              ndkVersion = "29.0.14206865"; # NDK r29 (stable LTS for API 37)
              androidComposition = pkgs.androidenv.composeAndroidPackages {
                ndkVersions = [ ndkVersion ];
                includeNDK = true;
                includeEmulator = true;
                includeSystemImages = true;
                platformVersions = [
                  "36"
                  "33"
                ];
                buildToolsVersions = [
                  "36.1.0"
                  "34.0.0"
                ];
                cmakeVersions = [ "3.22.1" ];
                abiVersions = [
                  "arm64-v8a"
                  "x86_64"
                ];
              };
              androidSdk = androidComposition.androidsdk;

              jdk = pkgs.jdk17;
            in
            {
              packages =
                with pkgs;
                [
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
                ]
                ++ (
                  if type == "android" then
                    with pkgs;
                    [
                      androidSdk
                      jdk
                      cmake
                      ninja
                    ]
                  else
                    [ ]
                );

              JEMALLOC_OVERRIDE = "${jemalloc-tikv}/lib/libjemalloc.a";
            }
            // (
              if type == "android" then
                {

                  JAVA_HOME = "${jdk.home}";
                  ANDROID_HOME = "${androidSdk}/libexec/android-sdk";
                  ANDROID_SDK_ROOT = "${androidSdk}/libexec/android-sdk";
                  ANDROID_NDK_ROOT = "${androidSdk}/libexec/android-sdk/ndk/${ndkVersion}";
                  NDK_HOME = "${androidSdk}/libexec/android-sdk/ndk/${ndkVersion}";
                  PATH = "$PATH:${androidSdk}/libexec/android-sdk/emulator:${androidSdk}/libexec/android-sdk/platform-tools";
                }
              else
                { }
            )
          );

      in
      {
        devShells = {
          default = mkShell "default";
          android = mkShell "android";
        };
      }
    );
}
