{ pkgs ? import <nixpkgs> {} }:

let
  wasiRustc = pkgs.pkgsCross.wasi32.buildPackages.rustc;
  e2ePython = pkgs.python3.withPackages (pythonPackages: [
    (pythonPackages.dogtail.overridePythonAttrs (old: {
      postPatch = (old.postPatch or "") + ''
        substituteInPlace dogtail/utils.py \
          --replace-fail "Settings(schema_id=a11yDConfKey)" "Settings.new(a11yDConfKey)" \
          --replace-fail "Settings(schema=a11yDConfKey)" "Settings.new(a11yDConfKey)"
      '';
    }))
    pythonPackages.selenium
  ]);
in
pkgs.mkShell {
  nativeBuildInputs = with pkgs; [
    cargo
    clippy
    dbus
    desktop-file-utils
    pkg-config
    wasiRustc
    rustfmt
    e2ePython
    lld
    util-linux
    wmctrl
    xclip
    xprop
    xorg-server
  ];

  buildInputs = with pkgs; [
    at-spi2-core
    gtk3
    gtk4
    webkitgtk_6_0
  ];

  # GTK 3 is required by Dogtail itself, while the production app uses GTK 4.
  # Make both accessibility typelibs explicit because the GTK 4 setup hook does
  # not propagate the GTK 3 or AT-SPI namespaces into GI_TYPELIB_PATH.
  shellHook = ''
    export GI_TYPELIB_PATH="${pkgs.gtk3}/lib/girepository-1.0:${pkgs.at-spi2-core}/lib/girepository-1.0''${GI_TYPELIB_PATH:+:$GI_TYPELIB_PATH}"
    export GTK_MODULES="gail:atk-bridge''${GTK_MODULES:+:$GTK_MODULES}"
  '';

  # WebKitWebDriver is supplied by webkitgtk_6_0. Xvfb, AT-SPI/Dogtail,
  # Selenium, and wmctrl support later headless native/browser E2E runs.
  WEBKIT_WEBDRIVER = "${pkgs.webkitgtk_6_0}/bin/WebKitWebDriver";
}
