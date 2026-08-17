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
          # From Cargo.toml, because a version written twice is a version
          # that goes stale in one of the two places: this said 0.5.2
          # through five releases, and nothing built from it was ever
          # named what it was.
          version = (nixpkgs.lib.importTOML ./Cargo.toml).package.version;
          src = self;
          cargoLock.lockFile = ./Cargo.lock;
          # ssh-keygen: the test suite drives it at build time, the binary
          # drives it at runtime.
          nativeBuildInputs = [ pkgs.openssh pkgs.makeWrapper ];
          # python3 names a mailbox from a key and assembles the frames
          # three tests corrupt on purpose. Tests only -- beb never runs
          # it -- so it stays out of the closure the binary carries.
          nativeCheckInputs = [ pkgs.python3 ];
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
