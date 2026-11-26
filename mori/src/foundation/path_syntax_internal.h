/* Internal lexical path operations. No allocation, I/O or retained pointers. */
#ifndef CBM_PATH_SYNTAX_INTERNAL_H
#define CBM_PATH_SYNTAX_INTERNAL_H

#include <stdbool.h>
#include <stddef.h>
#ifdef _WIN32
#include <wchar.h>
#endif

/* Join a slash-separated path to a repository directory, collapsing "." and
 * "..". A leading '\x01' makes v relative to the repository root. Returns
 * false if traversal would pass the root or the output does not fit. out must
 * hold sz writable bytes, with sz positive. */
bool cbm_repo_rel_join(const char *dir, const char *v, char *out, size_t sz);

enum { CBM_PATH_SYNTAX_MAX_BYTES = 4095 };
typedef enum {
    CBM_PATH_SYNTAX_OK = 0,
    CBM_PATH_SYNTAX_INVALID,
    CBM_PATH_SYNTAX_DEVICE_OR_TRIMMED,
    CBM_PATH_SYNTAX_COMPONENT_BYTE,
    CBM_PATH_SYNTAX_UTF8,
    CBM_PATH_SYNTAX_ROUNDTRIP,
    CBM_PATH_SYNTAX_NOT_ABSOLUTE,
    CBM_PATH_SYNTAX_INCOMPLETE_UNC
} cbm_path_syntax_result_t;

/* Byte spans have length 1..MAX_BYTES. No terminator is accessed by these two
 * functions. The component caller supplies one already-separated component;
 * relative-path structure (including slash/dot components) belongs to caller.
 * Device/trailing-dot-or-space rejection precedes forbidden-byte rejection.
 * Absolute checks preserve the ordinary drive/backslash-UNC lexical domain;
 * these functions do not establish native existence, aliases or permission. */
cbm_path_syntax_result_t cbm_path_windows_component(const unsigned char *component, size_t length);
cbm_path_syntax_result_t cbm_path_windows_absolute(const char *path, size_t length);

#ifdef _WIN32
/* source has exactly length non-NUL bytes followed by an accessible NUL;
 * length is 1..MAX_BYTES. Buffers are distinct, caller-owned writable spans.
 * Capacities are element counts, positive and <=INT_MAX; wide counts wchar_t.
 * Strict UTF8 -> wide -> UTF8 conversion preserves exact bytes including NUL.
 * INVALID is a bad pointer/length/capacity/terminated-span argument; UTF8 is a
 * forward conversion failure; ROUNDTRIP is reverse failure/length/byte mismatch.
 * Outputs may contain partial work on non-OK; only OK certifies conversion.
 * Calls are bounded, synchronous and retain neither input nor output storage. */
cbm_path_syntax_result_t cbm_path_windows_roundtrip(const char *source, size_t length,
                                                    wchar_t *wide, size_t wide_capacity,
                                                    char *roundtrip, size_t roundtrip_capacity);
#endif
#endif
