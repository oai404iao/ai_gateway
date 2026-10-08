#ifndef AI_GATEWAY_CONNECTOR_H
#define AI_GATEWAY_CONNECTOR_H

#include <stdint.h>

#define AI_GATEWAY_CONNECTOR_ABI_V1 1u
#define AI_GATEWAY_CONNECTOR_OK 0u
#define AI_GATEWAY_CONNECTOR_ERROR 1u
#define AI_GATEWAY_CONNECTOR_PANIC 2u
#define AI_GATEWAY_CONNECTOR_INVALID 3u

typedef struct {
    const uint8_t *ptr;
    uint64_t len;
} AiGatewayByteSlice;

typedef struct {
    uint8_t *ptr;
    uint64_t len;
} AiGatewayOwnedBuffer;

typedef struct {
    AiGatewayOwnedBuffer metadata;
    AiGatewayOwnedBuffer body;
} AiGatewayCallOutput;

typedef uint32_t (*AiGatewayDispatch)(
    AiGatewayByteSlice command,
    AiGatewayByteSlice metadata,
    AiGatewayByteSlice body,
    AiGatewayCallOutput *output
);
typedef void (*AiGatewayFreeBuffer)(AiGatewayOwnedBuffer buffer);

typedef struct {
    uint32_t abi_version;
    uint32_t struct_size;
    AiGatewayByteSlice manifest;
    AiGatewayDispatch dispatch;
    AiGatewayFreeBuffer free_buffer;
} AiGatewayConnectorDescriptor;

#ifdef __cplusplus
extern "C" {
#endif
const AiGatewayConnectorDescriptor *ai_gateway_connector_entry_v1(void);
#ifdef __cplusplus
}
#endif

#endif
