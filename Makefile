CC ?= cc
CFLAGS ?= -O2 -g -fPIC -Wall -Wextra -Wpedantic
LDFLAGS ?=
TARGET := libigpu_linear_bridge.so
SOURCE := src/gbm_fallback.c

.PHONY: all clean
all: $(TARGET)

$(TARGET): $(SOURCE)
	$(CC) $(CFLAGS) -shared -Wl,-z,defs -Wl,-soname,$(TARGET) $(LDFLAGS) -o $@ $< -ldl

clean:
	rm -f $(TARGET)
