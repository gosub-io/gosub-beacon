# Gosub Beacon — GTK browser

This is a prototype of a GTK browser written in Rust. It is a work in progress and is not intended to be used as a real browser bur merely as a test to see how well the Gosub engine can be implemented in a real-life applicaiton.

Do not expect anything to work. The browser is not functional at the moment.

![Screenshot of the browser](./docs/screenshots/initial-screen.png)

[More screenshots](./docs/screenshots/)

### Installing dependencies:

```bash
sudo apt install libgtk-4-dev
```

or similar on your linux system. There is no support for anything non-debian/ubuntu at the 
moment, but it should be easy to add support for other linux systems.

Currently, there is no macOS or Windows support. However, it should be possible to run the browser under WSL2.

Any help with adding support for other systems is welcome.

### Running the browser:

```bash
cargo run
```
