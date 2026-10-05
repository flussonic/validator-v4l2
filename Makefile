CARGO ?= cargo
PREFIX ?= /usr/local
.PHONY: all test check install clean
all:
	$(CARGO) build --release --offline
	cp target/release/validator-v4l2 validator-v4l2
test:
	$(CARGO) test --offline
	@if [ "$$(uname -s)" = Linux ]; then \
	    mkdir -p target; \
	    $(CC) -std=gnu11 -Wall -Wextra -Iinclude tests/native_events.c -o target/native-events && \
	    target/native-events && \
	    $(CC) -std=gnu11 -Wall -Wextra -Iinclude tests/native_buffers.c -o target/native-buffers && \
	    target/native-buffers; \
	fi
check:
	$(CARGO) fmt -- --check
	$(CARGO) clippy --offline --all-targets -- -D warnings
install: all
	install -Dm755 validator-v4l2 $(DESTDIR)$(PREFIX)/bin/validator-v4l2
clean:
	$(CARGO) clean
	rm -f validator-v4l2
