all:
	cargo run && llc -o output.o -filetype=obj -exception-model=default output.ll
