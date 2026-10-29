{
  description = "anchor-escrow dev shell";

  inputs.nixpkgs.url = "github:NixOS/nixpkgs/nixos-unstable";

  outputs = { nixpkgs, ... }:
    let
      systems = [ "x86_64-linux" "aarch64-linux" "x86_64-darwin" "aarch64-darwin" ];
      forAllSystems = f: nixpkgs.lib.genAttrs systems (system: f nixpkgs.legacyPackages.${system});
    in
    {
      devShells = forAllSystems (pkgs: {
        default = pkgs.mkShell {
          # cargo-build-sbf (from solana-cli) fetches Solana's own platform tools on first build.
          packages = with pkgs; [ cargo rustc clippy rustfmt solana-cli anchor ];
        };
      });
    };
}
