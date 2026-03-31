build:
	cargo build --release
	cp ./target/release/liblttw.dylib ./testing_plugin/lttw/lua/lttw.so
