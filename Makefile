all:
	cargo run && clang -c -fno-unwind-tables -fno-exceptions output.ll
