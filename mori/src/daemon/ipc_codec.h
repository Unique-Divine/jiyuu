/* Private byte codec entry points for the daemon transport. */
#ifndef CBM_DAEMON_IPC_CODEC_H
#define CBM_DAEMON_IPC_CODEC_H

#include "daemon/ipc.h"

bool cbm_daemon_frame_header_decode_bounded(const uint8_t header[CBM_DAEMON_FRAME_HEADER_SIZE],
                                           uint32_t max_payload_length, cbm_daemon_frame_t *frame);

bool mori_ipc_instance_key_valid(const uint8_t key[CBM_DAEMON_KEY_SIZE - 1U]);
bool mori_ipc_frame_header_encode(uint8_t out[CBM_DAEMON_FRAME_HEADER_SIZE], int kind,
                                  uint16_t flags, uint32_t length);
bool mori_ipc_frame_header_decode(const uint8_t header[CBM_DAEMON_FRAME_HEADER_SIZE],
                                  uint32_t max_payload_length, uint8_t *kind_out,
                                  uint16_t *flags_out, uint32_t *length_out);

#define MORI_IPC_IO_RETRY (-2)

/* Transfer callbacks borrow the connection through the synchronous call.
 * io_once returns bytes moved, 0 for timeout, -1 for EOF/error or -2 to
 * retry. Frame limits keep each length and byte count within int and DWORD.
 * The native allocator owns payloads that cross the ABI back to C. */
typedef struct {
    int (*io_once)(cbm_daemon_ipc_connection_t *, void *, size_t, uint64_t, bool writing);
    bool (*is_poisoned)(cbm_daemon_ipc_connection_t *);
    void (*mark_written)(const void *, size_t);
    void (*poison)(cbm_daemon_ipc_connection_t *);
    void *(*alloc)(size_t);
    void (*free)(void *);
} mori_ipc_transfer_ops_t;

typedef struct {
    uint8_t kind;
    uint16_t flags;
    uint32_t length;
    uint8_t *payload;
} mori_ipc_received_frame_t;

bool mori_ipc_send(cbm_daemon_ipc_connection_t *connection, const mori_ipc_transfer_ops_t *ops,
                   uint64_t deadline, int kind, uint16_t flags, const void *payload,
                   uint32_t length);
int mori_ipc_receive(cbm_daemon_ipc_connection_t *connection, const mori_ipc_transfer_ops_t *ops,
                    uint64_t deadline, uint32_t max_length, bool after_failed_send,
                    mori_ipc_received_frame_t *out);

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
