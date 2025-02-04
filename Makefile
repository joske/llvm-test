all:
	cargo run && llc -march=riscv32 -o output.o -filetype=obj -exception-model=default output.ll
