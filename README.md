# bitfix
Simple Lua-scriptable runtime binary patcher

## building
```shell
$ cargo build --release
```

## usage
Copy `target/release/bitfix.dll` next to the exe to be patched and name it something that will get loaded, such as `d3d9.dll` or `x3daudio1_7.dll`.
Lua patches will be loaded from a `bitfix/` directory.
