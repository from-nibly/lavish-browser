{ pkgs ? import <nixpkgs> {} }:

let
  wasiRustc = pkgs.pkgsCross.wasi32.buildPackages.rustc;
  e2ePython = pkgs.python3.withPackages (pythonPackages: with pythonPackages; [
    dogtail
    selenium
  ]);
in
pkgs.mkShell {
  nativeBuildInputs = with pkgs; [
    cargo
    clippy
    pkg-config
    wasiRustc
    rustfmt
    e2ePython
    lld
    at-spi2-core
    wmctrl
    xorg-server
  ];

  buildInputs = with pkgs; [
    gtk4
    webkitgtk_6_0
  ];

  # WebKitWebDriver is supplied by webkitgtk_6_0. Xvfb, AT-SPI/Dogtail,
  # Selenium, and wmctrl support later headless native/browser E2E runs.
  WEBKIT_WEBDRIVER = "${pkgs.webkitgtk_6_0}/bin/WebKitWebDriver";
}
