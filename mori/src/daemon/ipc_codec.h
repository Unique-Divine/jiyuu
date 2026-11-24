/* Private byte codec entry points for the daemon transport. */
#ifndef CBM_DAEMON_IPC_CODEC_H
#define CBM_DAEMON_IPC_CODEC_H

#include "daemon/daemon.h"

bool cbm_daemon_frame_header_decode_bounded(const uint8_t header[CBM_DAEMON_FRAME_HEADER_SIZE],
                                           uint32_t max_payload_length, cbm_daemon_frame_t *frame);

#ifdef CBM_WITH_RUST_IPC
bool mori_ipc_instance_key_valid(const uint8_t key[CBM_DAEMON_KEY_SIZE - 1U]);
bool mori_ipc_frame_header_encode(uint8_t out[CBM_DAEMON_FRAME_HEADER_SIZE], int kind,
                                  uint16_t flags, uint32_t length);
bool mori_ipc_frame_header_decode(const uint8_t header[CBM_DAEMON_FRAME_HEADER_SIZE],
                                  uint32_t max_payload_length, uint8_t *kind_out,
                                  uint16_t *flags_out, uint32_t *length_out);

#ifndef _WIN32
typedef struct {
    uint64_t device;
    uint64_t inode;
    int64_t ctime_seconds;
    uint32_t ctime_nanoseconds;
} mori_ipc_socket_identity_t;

bool mori_ipc_socket_record_encode(const uint8_t magic[8],
                                   const mori_ipc_socket_identity_t *identity,
                                   const uint8_t *anchor, size_t anchor_length, uint8_t *out);
bool mori_ipc_socket_record_decode(const uint8_t magic[8], const uint8_t *bytes,
                                   mori_ipc_socket_identity_t *identity_out, char *anchor_out);
#endif
#endif
#endif
