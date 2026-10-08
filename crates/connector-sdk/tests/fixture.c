#include "../include/ai_gateway_connector.h"
#include <stdlib.h>
#include <string.h>

#ifndef FIXTURE_ABI
#define FIXTURE_ABI AI_GATEWAY_CONNECTOR_ABI_V1
#endif
#ifndef FIXTURE_BODY_MODE
#define FIXTURE_BODY_MODE 0
#endif

static AiGatewayOwnedBuffer copy(const void *bytes, uint64_t length) {
    AiGatewayOwnedBuffer result = {NULL, 0};
    if (length != 0) {
        result.ptr = malloc((size_t)length);
        if (result.ptr == NULL) abort();
        memcpy(result.ptr, bytes, (size_t)length);
        result.len = length;
    }
    return result;
}

static void release(AiGatewayOwnedBuffer buffer) {
    free(buffer.ptr);
}

static int equals(AiGatewayByteSlice bytes, const char *value) {
    size_t length = strlen(value);
    return bytes.len == length && memcmp(bytes.ptr, value, length) == 0;
}

static uint32_t dispatch(
    AiGatewayByteSlice command,
    AiGatewayByteSlice metadata,
    AiGatewayByteSlice body,
    AiGatewayCallOutput *output
) {
    memset(output, 0, sizeof(*output));
#ifdef FIXTURE_SETTINGS
    if (equals(command, "settings.describe/v1")) {
        const char value[] = "{\"schema_version\":1,\"title\":{\"en\":\"Generic fixture\"},\"fields\":[{\"key\":\"mode\",\"label\":{\"en\":\"Mode\"},\"required\":true,\"type\":\"string\",\"max_length\":32}],\"defaults\":{\"mode\":\"default\"}}";
        output->metadata = copy(value, sizeof(value) - 1);
        return AI_GATEWAY_CONNECTOR_OK;
    }
    if (equals(command, "settings.validate/v1")) {
        const char value[] = "{\"valid\":true,\"errors\":[]}";
        output->metadata = copy(value, sizeof(value) - 1);
        return AI_GATEWAY_CONNECTOR_OK;
    }
    if (equals(command, "settings.compile/v1")) {
        char *input = malloc((size_t)metadata.len + 1);
        if (input == NULL) abort();
        memcpy(input, metadata.ptr, (size_t)metadata.len);
        input[metadata.len] = '\0';
        const char *value = strstr(input, "alternate") != NULL
            ? "{\"config\":{\"mode\":\"alternate\"}}" : "{\"config\":{\"mode\":\"default\"}}";
        output->metadata = copy(value, strlen(value));
        free(input);
        return AI_GATEWAY_CONNECTOR_OK;
    }
    if (equals(command, "settings.migrate/v1")) {
        const char value[] = "{\"schema_version\":1,\"values\":{\"mode\":\"default\"}}";
        output->metadata = copy(value, sizeof(value) - 1);
        return AI_GATEWAY_CONNECTOR_OK;
    }
#endif
    if (command.len == 5 && memcmp(command.ptr, "error", 5) == 0) {
        const char error[] = "{\"code\":\"fixture_error\",\"message\":\"secret\"}";
        output->metadata = copy(error, sizeof(error) - 1);
        return AI_GATEWAY_CONNECTOR_ERROR;
    }
    if (command.len == 9 && memcmp(command.ptr, "malformed", 9) == 0) {
        output->metadata.len = UINT64_MAX;
        return AI_GATEWAY_CONNECTOR_OK;
    }
    if (equals(command, "attempt.capabilities")) {
        const char value[] = "{\"successful_response_is_sse\":true}";
        output->metadata = copy(value, sizeof(value) - 1);
        return AI_GATEWAY_CONNECTOR_OK;
    }
    if (equals(command, "attempt.target")) {
        const char value[] = "{\"url\":\"https://upstream.test/base/responses\"}";
        output->metadata = copy(value, sizeof(value) - 1);
        return AI_GATEWAY_CONNECTOR_OK;
    }
    if (equals(command, "attempt.headers")) {
        const char value[] = "{\"set\":{\"x-provider\":\"fixture\",\"authorization\":\"plugin-value\",\"content-type\":\"application/json\"},\"remove\":[\"x-remove\"]}";
        output->metadata = copy(value, sizeof(value) - 1);
        return AI_GATEWAY_CONNECTOR_OK;
    }
    if (equals(command, "attempt.body")) {
        const char value[] = "{}";
        output->metadata = copy(value, sizeof(value) - 1);
        output->body = copy(body.ptr, body.len);
        return AI_GATEWAY_CONNECTOR_OK;
    }
    if (equals(command, "attempt.image_edit_plan")) {
#if FIXTURE_BODY_MODE == 1
        const char value[] = "{\"body_mode\":\"unknown\"}";
#elif FIXTURE_BODY_MODE == 2
        const char value[] = "{\"body_mode\":\"json_base64\",\"prefix_bytes\":0}";
#else
        const char value[] = "{\"body_mode\":\"replay_multipart\"}";
#endif
        output->metadata = copy(value, sizeof(value) - 1);
        return AI_GATEWAY_CONNECTOR_OK;
    }
    output->metadata = copy(metadata.ptr, metadata.len);
    output->body = copy(body.ptr, body.len);
    return AI_GATEWAY_CONNECTOR_OK;
}

static const char manifest[] =
    "{\"id\":\"fixture\",\"version\":\"1.0.0\","
#ifdef FIXTURE_SETTINGS
    "\"protocol_version\":2,"
#endif
    "\"operations\":[\"responses\",\"responses-ws\",\"images_edit\"],"
#ifdef FIXTURE_EMPTY_COMMANDS
    "\"commands\":[]}";
#else
    "\"commands\":[\"echo\",\"error\",\"malformed\","
#ifdef FIXTURE_SETTINGS
    "\"settings.describe/v1\",\"settings.validate/v1\",\"settings.compile/v1\",\"settings.migrate/v1\","
#endif
    "\"attempt.body\",\"attempt.target\","
#ifndef FIXTURE_OMIT_HEADERS
    "\"attempt.headers\","
#endif
    "\"attempt.capabilities\",\"attempt.image_edit_plan\",\"attempt.image_part_plan\"]}";
#endif

static const AiGatewayConnectorDescriptor descriptor = {
    FIXTURE_ABI,
    sizeof(AiGatewayConnectorDescriptor),
    {(const uint8_t *)manifest, sizeof(manifest) - 1},
    dispatch,
    release,
};

const AiGatewayConnectorDescriptor *ai_gateway_connector_entry_v1(void) {
    return &descriptor;
}
