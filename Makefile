CARGO ?= cargo
PREFIX ?= /usr/local
.PHONY: all test check install clean
all:
	$(CARGO) build --release --offline
	cp target/release/validator-v4l2 validator-v4l2
test:
	$(CARGO) test --offline
check:
	$(CARGO) fmt -- --check
	$(CARGO) clippy --offline --all-targets -- -D warnings
install: all
	install -Dm755 validator-v4l2 $(DESTDIR)$(PREFIX)/bin/validator-v4l2
clean:
	$(CARGO) clean
	rm -f validator-v4l2
