{ pkgs ? import <nixpkgs> {} }:

pkgs.mkShell {
  nativeBuildInputs = with pkgs; [
    cargo
    pkg-config
    rustc
    rustfmt
  ];

  buildInputs = with pkgs; [
    gtk4
    webkitgtk_6_0
  ];
}
