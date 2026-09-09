{
  description = "Daedalus — homelab control plane: Rust development environment";

  inputs = {
    nixpkgs.url = "github:NixOS/nixpkgs/nixos-unstable";
    rust-overlay = {
      url = "github:oxalica/rust-overlay";
      inputs.nixpkgs.follows = "nixpkgs";
    };
  };

  outputs = { self, nixpkgs, rust-overlay }:
    let
      systems = [ "aarch64-darwin" "x86_64-darwin" "aarch64-linux" "x86_64-linux" ];

      forAllSystems = f: nixpkgs.lib.genAttrs systems (system:
        f (import nixpkgs {
          inherit system;
          overlays = [ (import rust-overlay) ];
        }));
    in
    {
      devShells = forAllSystems (pkgs:
        let
          inherit (pkgs) lib stdenv;

          # Phase 0 pins the toolchain in rust-toolchain.toml. Until that file
          # exists, track stable — and pick it up automatically once it does,
          # so the flake and rustup never disagree about the version.
          rustToolchain =
            if builtins.pathExists ./rust-toolchain.toml
            then pkgs.rust-bin.fromRustupToolchainFile ./rust-toolchain.toml
            else
              pkgs.rust-bin.stable.latest.default.override {
                extensions = [ "rust-src" "rust-analyzer" "clippy" "rustfmt" "llvm-tools" ];
              };

          # Build inputs for the crates the design calls for: git2 vendors
          # libgit2 and OpenSSL (cmake + perl), sqlx needs SQLite, and reqwest
          # is configured for rustls so no system TLS is required.
          nativeBuild = with pkgs; [ pkg-config cmake perl ];
          systemLibs = with pkgs; [ openssl sqlite zlib ]
            # Rust on Darwin still wants libiconv at link time.
            ++ lib.optionals stdenv.hostPlatform.isDarwin [ libiconv ];

          rustTools = with pkgs; [
            # the test runner the plan standardises on
            cargo-nextest
            cargo-watch
            # nicer watcher for a long edit loop
            bacon
            # licence + advisory policy, run in CI
            cargo-deny
            cargo-audit
            # snapshot review for plan output
            cargo-insta
            cargo-edit
            # see what maud!/derive actually generate
            cargo-expand
            cargo-outdated
            # unused dependency detection
            cargo-machete
            cargo-llvm-cov
            # phase 9 profiling
            cargo-flamegraph
            # migrations + the offline query cache
            sqlx-cli
            just
          ];

          kubeTools = with pkgs; [
            kubectl
            kubernetes-helm
            # phase 7 bootstraps this into each cluster
            fluxcd
            k9s
            kubectx
            stern
            # phase 8 groundwork
            talosctl
          ];

          infraTools = with pkgs; [
            # tenant secrets, age-encrypted in git
            sops
            age
            ssh-to-age
            openssh
            # dae graph --format dot | dot -Tpng
            graphviz
          ];

          apiTools = with pkgs; [
            curl
            # ergonomic HTTP client for poking the Proxmox API
            xh
            jq
            yq-go
          ];

          gitTools = with pkgs; [
            git
            gh
            lazygit
            delta
            pre-commit
          ];

          lintTools = with pkgs; [
            # spell-check across code and the docs/ tree
            typos
            markdownlint-cli2
            nixpkgs-fmt
            # nix language server
            nil
          ];

          shellTools = with pkgs; [
            zsh
            oh-my-zsh
            fzf
            # fzf's file/dir backend
            fd
            ripgrep
            # fzf previews
            bat
            eza
            zoxide
            direnv
            zsh-autosuggestions
            zsh-syntax-highlighting
            zsh-history-substring-search
            zsh-fzf-tab
          ];

          zdotdir = import ./nix/zsh.nix { inherit pkgs; };

          commonPackages =
            [ rustToolchain ]
            ++ nativeBuild ++ systemLibs
            ++ rustTools ++ kubeTools ++ infraTools
            ++ apiTools ++ gitTools ++ lintTools ++ shellTools;

          commonEnv = ''
            # Writable home for anything that cannot use the read-only store.
            export DAEDALUS_STATE="$PWD/.direnv/daedalus"
            mkdir -p "$DAEDALUS_STATE"

            export RUST_BACKTRACE=1
            export RUST_SRC_PATH="${rustToolchain}/lib/rustlib/src/rust/library"

            # sqlx reads DATABASE_URL for `cargo sqlx` and the query! macros.
            export DATABASE_URL="sqlite://$PWD/local/daedalus.db"
            mkdir -p "$PWD/local"

            # git2 is configured to vendor libgit2 and OpenSSL; point the build
            # scripts at the nix copies so they do not compile their own.
            export OPENSSL_DIR="${pkgs.openssl.dev}"
            export OPENSSL_LIB_DIR="${pkgs.openssl.out}/lib"
            export OPENSSL_NO_VENDOR=1
            export PKG_CONFIG_PATH="${pkgs.openssl.dev}/lib/pkgconfig:${pkgs.sqlite.dev}/lib/pkgconfig''${PKG_CONFIG_PATH:+:$PKG_CONFIG_PATH}"
          '';
        in
        {
          # `nix develop` — interactive zsh. `nix develop -c <cmd>` stays in
          # bash, which is what CI, direnv and scripts want.
          default = pkgs.mkShell {
            name = "daedalus";
            packages = commonPackages;

            shellHook = commonEnv + ''
              # $- contains 'i' only when nix started an interactive bash, so
              # `nix develop -c cargo build` and direnv never land in zsh.
              if [[ $- == *i* && -z ''${DAEDALUS_IN_ZSH:-} ]]; then
                export DAEDALUS_IN_ZSH=1
                export ZDOTDIR="${zdotdir}"
                export SHELL="${pkgs.zsh}/bin/zsh"
                exec "${pkgs.zsh}/bin/zsh"
              fi
            '';
          };

          # Same toolchain, plain bash. For CI, or if you dislike the theme.
          bash = pkgs.mkShell {
            name = "daedalus-bash";
            packages = commonPackages;
            shellHook = commonEnv + ''
              echo "daedalus dev shell (bash) — $(rustc --version)"
            '';
          };

          # Just enough to compile: no editors, no kube tooling. Faster to
          # realise on a cold cache, which matters in CI.
          ci = pkgs.mkShell {
            name = "daedalus-ci";
            packages = [ rustToolchain pkgs.cargo-nextest pkgs.cargo-deny pkgs.just ]
              ++ nativeBuild ++ systemLibs;
            shellHook = commonEnv;
          };
        });

      formatter = forAllSystems (pkgs: pkgs.nixpkgs-fmt);
    };
}
