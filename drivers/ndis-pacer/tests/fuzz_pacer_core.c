/* libFuzzer target for the pacer's frame parsers (dasio#27, A123).
 *
 * The filter runs these on every frame the NIC sends or receives and on
 * the dasio service's IOCTL input, in the kernel: a read past the frame
 * there is a blue screen. libFuzzer hands each input in a heap buffer of
 * exactly its size, so AddressSanitizer stops at the first byte read past
 * it. Next to memory safety the target checks what each parser promises:
 *
 * - pacer_classify and pacer_rx_defer answer 0 or 1 and read only the
 *   first `len` bytes (the answer on a shorter prefix never needs bytes
 *   past it, checked by giving the prefix its own exact buffer);
 * - pacer_tx_next walks the packed input forward, every frame inside the
 *   input and within PACER_TX_MIN_FRAME..PACER_TX_MAX_FRAME, and leaves
 *   `*at` where it was on a malformed input;
 * - a frame pacer_ktx_frame builds from fuzzed fields is a valid IPv4
 *   frame: its header checksum verifies, pacer_tx_next accepts it packed,
 *   and pacer_classify calls it priority exactly for DSCP 46 or 56 or a
 *   PTP port.
 *
 * Built with clang -fsanitize=fuzzer,address,undefined (CI, Linux). */
#include <stdint.h>
#include <stdlib.h>
#include <string.h>

#include "../core/pacer_core.h"

static void require(int cond)
{
    if (!cond) {
        abort();
    }
}

/* `data[0..len)` copied into a buffer of exactly `len` bytes, so a read
 * past `len` is caught. */
static uint8_t *exact(const uint8_t *data, size_t len)
{
    uint8_t *b = malloc(len ? len : 1);
    require(b != NULL);
    if (len) {
        memcpy(b, data, len);
    }
    return b;
}

static void parsers(const uint8_t *data, size_t size)
{
    uint32_t len = (uint32_t)size;
    enum pacer_class c = pacer_classify(data, len);
    int d = pacer_rx_defer(data, len);
    require(c == PACER_OTHER || c == PACER_PRIORITY);
    require(d == 0 || d == 1);

    /* Every prefix's answer comes from its own bytes only. */
    if (size > 0) {
        size_t cut = data[0] % (size + 1);
        uint8_t *p = exact(data, cut);
        enum pacer_class pc = pacer_classify(p, (uint32_t)cut);
        int pd = pacer_rx_defer(p, (uint32_t)cut);
        require(pc == PACER_OTHER || pc == PACER_PRIORITY);
        require(pd == 0 || pd == 1);
        free(p);
    }

    {
        uint32_t at = 0;
        int frames = 0;
        for (;;) {
            uint32_t before = at;
            uint32_t off = 0;
            uint32_t flen = 0;
            int r = pacer_tx_next(data, len, &at, &off, &flen);
            require(r == 1 || r == 0 || r == -1);
            if (r != 1) {
                require(at == before);
                break;
            }
            require(flen >= PACER_TX_MIN_FRAME && flen <= PACER_TX_MAX_FRAME);
            require(off >= before + 2 && off <= len && flen <= len - off);
            require(at == off + flen);
            /* A frame it accepts is safe to classify on its own. */
            {
                uint8_t *f = exact(data + off, flen);
                enum pacer_class fc = pacer_classify(f, flen);
                require(fc == PACER_OTHER || fc == PACER_PRIORITY);
                free(f);
            }
            frames++;
            require(frames <= (int)(len / (2 + PACER_TX_MIN_FRAME)) + 1);
        }
    }

    if (size >= 20) {
        uint32_t hlen = 20u + 4u * (data[0] % 11u);
        if (hlen <= len) {
            (void)pacer_ipv4_checksum(data, hlen);
        }
    }
}

/* One pacer_ktx_frame built from the input's first bytes. */
static void built_frame(const uint8_t *data, size_t size)
{
    struct pacer_ktx_addr a;
    uint8_t frame[PACER_KTX_HEADER + PACER_KTX_MAX_PAYLOAD];
    uint8_t packed[2 + sizeof frame];
    uint32_t payload;
    uint32_t n;
    uint64_t seq = 0;
    uint64_t t = 0;
    uint16_t ip_id;

    if (size < 34) {
        return;
    }
    memcpy(a.dst_mac, data, 6);
    memcpy(a.src_mac, data + 6, 6);
    memcpy(a.dst_ip, data + 12, 4);
    memcpy(a.src_ip, data + 16, 4);
    a.dst_port = (uint16_t)((data[20] << 8) | data[21]);
    a.src_port = (uint16_t)((data[22] << 8) | data[23]);
    a.dscp = (uint8_t)(data[24] & 0x3F);
    payload = ((uint32_t)data[25] << 8 | data[26]) % (PACER_KTX_MAX_PAYLOAD + 32u);
    ip_id = (uint16_t)((data[27] << 8) | data[28]);
    memcpy(&seq, data + 29, 4);
    memcpy(&t, data + 30, 4);

    n = pacer_ktx_frame(frame, &a, payload, ip_id, seq, t);
    if (payload < PACER_KTX_MIN_PAYLOAD || payload > PACER_KTX_MAX_PAYLOAD) {
        require(n == 0);
        return;
    }
    require(n == PACER_KTX_HEADER + payload);

    /* The IPv4 header (bytes 14..33) sums to 0xFFFF with its checksum in. */
    {
        uint8_t hdr[20];
        uint16_t sum;
        memcpy(hdr, frame + 14, 20);
        sum = (uint16_t)((hdr[10] << 8) | hdr[11]);
        hdr[10] = 0;
        hdr[11] = 0;
        require(pacer_ipv4_checksum(hdr, 20) == sum);
    }

    {
        int ptp = a.dst_port == 319u || a.dst_port == 320u;
        int prio = a.dscp == PACER_DSCP_EF || a.dscp == PACER_DSCP_CS7 || ptp;
        require(pacer_classify(frame, n) == (prio ? PACER_PRIORITY : PACER_OTHER));
    }

    {
        uint32_t at = 0;
        uint32_t off = 0;
        uint32_t flen = 0;
        packed[0] = (uint8_t)(n & 0xFF);
        packed[1] = (uint8_t)(n >> 8);
        memcpy(packed + 2, frame, n);
        if (n >= PACER_TX_MIN_FRAME && n <= PACER_TX_MAX_FRAME) {
            require(pacer_tx_next(packed, 2 + n, &at, &off, &flen) == 1);
            require(off == 2 && flen == n && at == 2 + n);
            require(pacer_tx_next(packed, 2 + n, &at, &off, &flen) == 0);
        }
    }
}

int LLVMFuzzerTestOneInput(const uint8_t *data, size_t size)
{
    parsers(data, size);
    built_frame(data, size);
    return 0;
}
