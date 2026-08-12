{
  description = "beb: signed messages between identities";

  inputs.nixpkgs.url = "github:NixOS/nixpkgs/nixpkgs-unstable";

  outputs = { self, nixpkgs }:
    let
      systems = [ "aarch64-darwin" "x86_64-darwin" "aarch64-linux" "x86_64-linux" ];
      forAll = f: nixpkgs.lib.genAttrs systems (system: f nixpkgs.legacyPackages.${system});
    in
    {
      packages = forAll (pkgs: rec {
        default = beb;
        beb = pkgs.rustPlatform.buildRustPackage {
          pname = "beb";
          version = "0.4.0";
          src = self;
          cargoLock.lockFile = ./Cargo.lock;
          # ssh-keygen: the test suite drives it at build time, the binary
          # drives it at runtime.
          nativeBuildInputs = [ pkgs.openssh pkgs.makeWrapper ];
          postInstall = ''
            wrapProgram $out/bin/beb \
              --prefix PATH : ${pkgs.lib.makeBinPath [ pkgs.openssh ]}
          '';
        };
      });

      devShells = forAll (pkgs: {
        default = pkgs.mkShell {
          packages = [ pkgs.cargo pkgs.rustc pkgs.openssh ];
        };
      });
    };
}
