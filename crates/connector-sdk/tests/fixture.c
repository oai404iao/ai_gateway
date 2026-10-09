#include "../include/ai_gateway_connector.h"
#include <stdio.h>
#include <stdlib.h>
#include <string.h>

#ifndef FIXTURE_ABI
#define FIXTURE_ABI AI_GATEWAY_CONNECTOR_ABI_V1
#endif
#ifndef FIXTURE_BODY_MODE
#define FIXTURE_BODY_MODE 0
#endif
#ifndef FIXTURE_VERSION
#define FIXTURE_VERSION "1.0.0"
#endif
#ifndef FIXTURE_DESCRIPTOR_MODE
#define FIXTURE_DESCRIPTOR_MODE 0
#endif
#ifndef FIXTURE_RESPONSE_RESULT_MODE
#define FIXTURE_RESPONSE_RESULT_MODE 0
#endif
#ifndef FIXTURE_TARGET_URL
#define FIXTURE_TARGET_URL "https://upstream.test/base/responses"
#endif
#ifdef FIXTURE_COUNT_DESCRIBE
static unsigned descriptor_calls = 0;
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
#ifdef FIXTURE_PROTOCOL3
    if (equals(command, "response.json/v1")) {
        output->metadata = copy("{}", 2);
#if FIXTURE_RESPONSE_RESULT_MODE == 1
        output->body = copy("{invalid", 8);
#elif FIXTURE_RESPONSE_RESULT_MODE == 2
        const char value[] = "{\"id\":\"resp_fixture\",\"object\":\"response\",\"output\":[{\"type\":\"message\",\"role\":\"assistant\",\"content\":[{\"type\":\"output_text\",\"text\":\"raw-text\"}]}],\"usage\":{\"input_tokens\":999,\"output_tokens\":3,\"input_tokens_details\":{\"cached_tokens\":2},\"output_tokens_details\":{\"reasoning_tokens\":1}}}";
        output->body = copy(value, sizeof(value) - 1);
#elif FIXTURE_RESPONSE_RESULT_MODE == 3
        const char value[] = "{\"id\":\"resp_fixture\",\"object\":\"response\",\"output\":[{\"type\":\"message\",\"role\":\"assistant\",\"content\":[{\"type\":\"output_text\",\"text\":\"normalized\"}]}],\"usage\":{\"input_tokens\":9,\"output_tokens\":3,\"input_tokens_details\":{\"cached_tokens\":2},\"output_tokens_details\":{\"reasoning_tokens\":1}}}";
        output->body = copy(value, sizeof(value) - 1);
#else
        output->body = copy(body.ptr, body.len);
#endif
        return AI_GATEWAY_CONNECTOR_OK;
    }
    if (equals(command, "attempt.describe/v1")) {
#ifdef FIXTURE_COUNT_DESCRIBE
        descriptor_calls++;
#endif
#if FIXTURE_DESCRIPTOR_MODE == 1
        const char value[] = "{\"capabilities\":{\"successful_response_is_sse\":true},\"protocols\":[]}";
#elif FIXTURE_DESCRIPTOR_MODE == 2
        const char value[] = "{\"capabilities\":{\"preserves_affinity_on_failure\":false,\"successful_response_is_sse\":false,\"changes_request_body\":false},\"protocols\":[{\"protocol\":\"non_stream\",\"response\":\"json\"},{\"protocol\":\"non_stream\",\"response\":\"json\"}]}";
#elif FIXTURE_DESCRIPTOR_MODE == 3
        const char value[] = "{\"capabilities\":{\"preserves_affinity_on_failure\":false,\"successful_response_is_sse\":false,\"changes_request_body\":false},\"protocols\":[{\"protocol\":\"unknown\",\"response\":\"passthrough\"}]}";
#elif FIXTURE_DESCRIPTOR_MODE == 4
        const char value[] = "{\"capabilities\":{\"preserves_affinity_on_failure\":false,\"successful_response_is_sse\":false,\"changes_request_body\":false},\"protocols\":[{\"protocol\":\"websocket\",\"response\":\"passthrough\"}]}";
#elif FIXTURE_DESCRIPTOR_MODE == 5
        const char value[] = "{\"capabilities\":{\"preserves_affinity_on_failure\":false,\"successful_response_is_sse\":false,\"changes_request_body\":false},\"protocols\":[{\"protocol\":\"sse\",\"response\":\"json\"}]}";
#elif FIXTURE_DESCRIPTOR_MODE == 6
        const char value[] = "{\"capabilities\":{\"preserves_affinity_on_failure\":false,\"successful_response_is_sse\":false,\"changes_request_body\":false},\"protocols\":[{\"protocol\":\"sse\",\"response\":\"sse\"}]}";
#elif FIXTURE_DESCRIPTOR_MODE == 8
        const char value[] = "{\"capabilities\":{\"preserves_affinity_on_failure\":false,\"successful_response_is_sse\":false,\"changes_request_body\":false},\"protocols\":[{\"protocol\":\"non_stream\",\"response\":\"unknown\"}]}";
#elif FIXTURE_DESCRIPTOR_MODE == 9
        const char value[] = "{\"capabilities\":{\"preserves_affinity_on_failure\":false,\"successful_response_is_sse\":false,\"changes_request_body\":false},\"protocols\":[{\"protocol\":\"non_stream\",\"response\":\"json\"},{\"protocol\":\"sse\",\"response\":\"sse\"}]}";
#else
        const char value[] = "{\"capabilities\":{\"preserves_affinity_on_failure\":false,\"successful_response_is_sse\":false,\"changes_request_body\":false},\"protocols\":[{\"protocol\":\"non_stream\",\"response\":\"json\"}]}";
#endif
        output->metadata = copy(value, sizeof(value) - 1);
#if FIXTURE_DESCRIPTOR_MODE == 7
        output->body = copy("secret", 6);
#endif
        return AI_GATEWAY_CONNECTOR_OK;
    }
#endif
#ifdef FIXTURE_COUNT_DESCRIBE
    if (equals(command, "echo")) {
        char value[64];
        int length = snprintf(value, sizeof(value), "{\"descriptor_calls\":%u}", descriptor_calls);
        if (length < 0 || (size_t)length >= sizeof(value)) abort();
        output->metadata = copy(value, (uint64_t)length);
        return AI_GATEWAY_CONNECTOR_OK;
    }
#endif
    if (equals(command, "attempt.target")) {
        const char value[] = "{\"url\":\"" FIXTURE_TARGET_URL "\"}";
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
    "{\"id\":\"fixture\",\"version\":\"" FIXTURE_VERSION "\","
#ifdef FIXTURE_PROTOCOL3
    "\"protocol_version\":3,"
#elif defined(FIXTURE_SETTINGS)
    "\"protocol_version\":2,"
#endif
#ifdef FIXTURE_PROTOCOL3
#ifdef FIXTURE_RESPONSE_OPERATION
    "\"operations\":[\"" FIXTURE_RESPONSE_OPERATION "\"],"
#else
    "\"operations\":[\"responses\"],"
#endif
#else
    "\"operations\":[\"responses\",\"responses-ws\",\"images_edit\"],"
#endif
#ifdef FIXTURE_EMPTY_COMMANDS
    "\"commands\":[]}";
#else
    "\"commands\":[\"echo\",\"error\",\"malformed\","
#ifdef FIXTURE_PROTOCOL3
#ifndef FIXTURE_OMIT_DESCRIBE
    "\"attempt.describe/v1\","
#endif
#ifndef FIXTURE_OMIT_RESPONSE_JSON
    "\"response.json/v1\","
#endif
#ifndef FIXTURE_OMIT_RESPONSE_EVENT
    "\"response.event/v1\","
#endif
#endif
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
