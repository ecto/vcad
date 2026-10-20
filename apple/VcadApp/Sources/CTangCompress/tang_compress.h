// C ABI of apple/VcadApp/tang-compress-ffi (built by build-tc-ffi.sh).
#pragma once
#include <stddef.h>
#include <stdint.h>

typedef struct TcField {
    float *data;      // len f32 values, row-major (z, y, x)
    size_t len;
    size_t dims[3];   // (z, y, x)
    uint64_t fnv1a;   // FNV-1a 64 over the f32 bit patterns
} TcField;

int32_t tc_decode_cpu(const uint8_t *bytes, size_t len, TcField *out, char **err);
void tc_field_free(TcField *field);
void tc_string_free(char *s);
