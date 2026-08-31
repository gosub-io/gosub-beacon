# Builds the C smoke test against the Rust cdylib. Its real job is to fail loudly when
# include/beacon.h and src/lib.rs drift apart -- the header is hand-written, and this is
# what keeps it honest.

ROOT    := ../..
TARGET  := $(ROOT)/target/debug
CFLAGS  := -Wall -Wextra -Werror -std=c11 -Iinclude
LDFLAGS := -L$(TARGET) -lbeacon -Wl,-rpath,$(abspath $(TARGET))

.PHONY: all lib run clean
all: $(TARGET)/beacon-smoke

lib:
	cd $(ROOT) && cargo build -p beacon-ffi

$(TARGET)/beacon-smoke: examples/smoke.c include/beacon.h lib
	$(CC) $(CFLAGS) examples/smoke.c -o $@ $(LDFLAGS)

run: $(TARGET)/beacon-smoke
	$(TARGET)/beacon-smoke $(URL)

clean:
	rm -f $(TARGET)/beacon-smoke
