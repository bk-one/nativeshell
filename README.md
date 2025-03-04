# NativeShell (Experimental embedder for Flutter)

![](https://nativeshell.dev/screenshot-dev.png "Screenshot")

## Features

- Leverages existing Flutter desktop embedder on each platform
- Unlike Flutter desktop embedders, NativeShell provides consistent platform agnostic API
- Multi-window support
- Window management
    - Adjusting window styles and geometry
    - Modal dialogs
    - Windows can be set to track content size and resize automatically when content changes
- Platform menus (popup menu, menu bar)
- Drag and Drop
- Written in Rust, Flutter build transparently integrated with cargo

## Status

- This is project in a very experimental stage

## Getting started

Prerequisites:

1. [Install Rust](https://www.rust-lang.org/tools/install)
2. [Install Flutter](https://flutter.dev/docs/get-started/install)
3. [Enable Flutter desktop support](https://flutter.dev/desktop#set-up)
4. Switch to Fluttter Master (`flutter channel master; flutter upgrade`)

Clone and run examples:

```
git clone https://github.com/nativeshell/examples.git
cd examples
cargo run
```

For more information read the [introductory post](http://localhost:8080/post/nativeshell-announcement/) or go to [nativeshell.dev](https://nativeshell.dev).

