{
  description = "memory — workspace typed-database kernel (redb + rkyv + version-skew guard)";

  inputs = {
    nixpkgs.url = "github:NixOS/nixpkgs/nixpkgs-unstable";
    flake-utils.url = "github:numtide/flake-utils";
    rust-build = {
      url = "github:LiGoldragon/rust-build";
      inputs.nixpkgs.follows = "nixpkgs";
    };
  };

  outputs = { self, nixpkgs, flake-utils, rust-build }:
    flake-utils.lib.eachDefaultSystem (system:
      let
        pkgs = import nixpkgs { inherit system; };
        # The cluster Rust toolchain is the newest nightly, pinned durably via
        # fenix's flake lock inside rust-build — bump rust-build (or its fenix)
        # to advance Rust, not a rolling-stable channel guarded by a per-crate
        # frozen hash. `fromToolchainFile` keeps the `{ file; }` arg for
        # call-compatibility; the channel/hash in it no longer drives the
        # toolchain, so this crate gets nightly by merely repinning rust-build.
        rust = rust-build.lib.${system}.fromToolchainFile pkgs {
          file = ./rust-toolchain.toml;
        };

        inherit (rust) craneLib toolchain;
        src = rust.cleanCargoSource ./.;
        commonArgs = {
          inherit src;
          strictDeps = true;
        };
        cargoArtifacts = craneLib.buildDepsOnly commonArgs;
        scriptApplication = name: script: pkgs.writeShellApplication {
          name = "memory-${name}";
          runtimeInputs = [
            toolchain
          ];
          text = ''
            exec "${script}" "$@"
          '';
        };
        testScript = scriptApplication "test" ./scripts/test;
        testDocScript = scriptApplication "test-doc" ./scripts/test-doc;
        testKernelSurfaceScript = scriptApplication "test-kernel-surface" ./scripts/test-kernel-surface;
        testNoLegacySurfaceScript = scriptApplication "test-no-legacy-surface" ./scripts/test-no-legacy-surface;
      in
      {
        packages = {
          default = craneLib.buildPackage (commonArgs // {
            inherit cargoArtifacts;
          });

          test = testScript;
          test-doc = testDocScript;
          test-kernel-surface = testKernelSurfaceScript;
          test-no-legacy-surface = testNoLegacySurfaceScript;
        };

        apps = {
          default = {
            type = "app";
            program = "${testScript}/bin/memory-test";
            meta.description = "Run memory's full test suite";
          };

          test = {
            type = "app";
            program = "${testScript}/bin/memory-test";
            meta.description = "Run memory's full test suite";
          };

          test-doc = {
            type = "app";
            program = "${testDocScript}/bin/memory-test-doc";
            meta.description = "Run memory's documentation tests";
          };

          test-kernel-surface = {
            type = "app";
            program = "${testKernelSurfaceScript}/bin/memory-test-kernel-surface";
            meta.description = "Run memory's typed-kernel integration tests";
          };

          test-no-legacy-surface = {
            type = "app";
            program = "${testNoLegacySurfaceScript}/bin/memory-test-no-legacy-surface";
            meta.description = "Run memory's retired-surface absence witnesses";
          };
        };

        checks = {
          # ─── Build ───────────────────────────────────────────
          # The library compiles (separate from running tests).
          build = craneLib.cargoBuild (commonArgs // {
            inherit cargoArtifacts;
          });

          # ─── Default test surface ─────────────────────────────
          # `cargo test` — runs every test target end to end.
          # Includes the kernel-surface tests and retired-surface
          # absence witnesses in one pass.
          test = craneLib.cargoTest (commonArgs // {
            inherit cargoArtifacts;
          });

          # ─── Per-file integration test runs ──────────────────
          # Each integration test file gets its own check so a
          # failure surfaces named, not buried.
          test-no-legacy-surface = craneLib.cargoTest (commonArgs // {
            inherit cargoArtifacts;
            cargoTestExtraArgs = "--test no_legacy_surface";
          });

          test-kernel-surface = craneLib.cargoTest (commonArgs // {
            inherit cargoArtifacts;
            cargoTestExtraArgs = "--test kernel";
          });

          # ─── Doc-tests ────────────────────────────────────────
          # The kernel's typed-Table example doctests in lib.rs
          # are `ignore`d (they reference types not in scope at
          # doc-test time) but the prose still has to compile to
          # valid markdown / valid rustdoc syntax. `cargo test
          # --doc` enforces that.
          test-doc = craneLib.cargoTest (commonArgs // {
            inherit cargoArtifacts;
            cargoTestExtraArgs = "--doc";
          });

          # ─── Documentation builds without warnings ────────────
          # rustdoc catches broken intra-doc links, missing
          # references, malformed `[`...`]` brackets in prose.
          # Memory's API surface IS partly documentation (the
          # `Schema` and `Table<K, V>` examples drive consumer
          # crates).
          doc = craneLib.cargoDoc (commonArgs // {
            inherit cargoArtifacts;
            RUSTDOCFLAGS = "-D warnings";
          });

          # ─── Formatter ────────────────────────────────────────
          fmt = craneLib.cargoFmt {
            inherit src;
          };

          # ─── Lint ─────────────────────────────────────────────
          # Clippy on the whole crate; warnings are errors.
          clippy = craneLib.cargoClippy (commonArgs // {
            inherit cargoArtifacts;
            cargoClippyExtraArgs = "--all-targets -- -D warnings";
          });
        };

        devShells.default = pkgs.mkShell {
          name = "memory";
          packages = [
            pkgs.jujutsu
            pkgs.pkg-config
            toolchain
          ];
        };
      }
    );
}
