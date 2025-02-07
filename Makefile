all:
	cargo run && llc -march=riscv32 -o output.o -filetype=obj output.ll
