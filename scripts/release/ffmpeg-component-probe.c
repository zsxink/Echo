/*
 * Runtime component probe for Echo's vendored libmpv/FFmpeg stack.
 *
 * This deliberately does not include FFmpeg headers: the release Gate runs on
 * machines without an FFmpeg SDK, and the question is what the shipped dylibs
 * register, not what a header would declare. The partial structs below mirror
 * only the public FFmpeg 6.0 prefixes the enumeration API exposes:
 *
 *   AVCodec        name, long_name, type          (libavcodec/codec.h, public)
 *   AVInputFormat  name, long_name                (libavformat/avformat.h, public)
 *   AVOutputFormat name, long_name, ...            (libavformat/avformat.h, public)
 *   AVFilter       name, description, inputs, outputs  (libavfilter/avfilter.h)
 *
 * Filter pads are read through avfilter_filter_pad_count() and
 * avfilter_pad_get_type(). In FFmpeg 6.0 the pad arrays are not NULL
 * terminated and AVFilterPad is private; walking the array directly is not a
 * stable or correct way to enumerate them. The public helpers also tell us the
 * real pad count, so a filter with zero pads is distinguishable from one whose
 * first pad name is NULL.
 *
 * Output is TSV so the checker can parse it without a JSON dependency:
 *   version     <v>
 *   config      <avcodec_configuration()>
 *   mpv         <mpv_client_api_version()>
 *   loaded      <symbol> <absolute path of the image that defines it>
 *   filter      <name> <input types|-> <output types|->
 *   decoder     <name> <AVMediaType>
 *   encoder     <name> <AVMediaType>
 *   demuxer     <name (may be a comma-separated alias list)>
 *   muxer       <name (may be a comma-separated alias list)>
 *   protocol_in <name>
 *   protocol_out<name>
 *   counts      <key=value pairs>
 *
 * The `loaded` rows prove the process actually loaded the dylibs under the
 * vendor directory rather than a Homebrew copy found earlier on the loader
 * path.
 */

#include <dlfcn.h>
#include <stddef.h>
#include <stdio.h>

/* Mirrors libavutil/avutil.h's AVMediaType with the real integer values: the
 * Gate host has no FFmpeg SDK, so the headers cannot be included, but the pad
 * types we print below must use the same numbers as the pinned source. */
typedef enum AVMediaType {
    AVMEDIA_TYPE_UNKNOWN = -1,
    AVMEDIA_TYPE_VIDEO,
    AVMEDIA_TYPE_AUDIO,
    AVMEDIA_TYPE_DATA,
    AVMEDIA_TYPE_SUBTITLE,
    AVMEDIA_TYPE_ATTACHMENT,
    AVMEDIA_TYPE_NB
} AVMediaType;

typedef struct AVCodec {
    const char *name;
    const char *long_name;
    int type;
} AVCodec;

typedef struct AVFilterPad AVFilterPad;

typedef struct AVFilter {
    const char *name;
    const char *description;
    const AVFilterPad *inputs;
    const AVFilterPad *outputs;
} AVFilter;

typedef struct AVInputFormat {
    const char *name;
    const char *long_name;
} AVInputFormat;

typedef struct AVOutputFormat {
    const char *name;
    const char *long_name;
    const char *mime_type;
    const char *extensions;
} AVOutputFormat;

extern const AVFilter *av_filter_iterate(void **opaque);
extern const AVCodec *av_codec_iterate(void **opaque);
extern const AVInputFormat *av_demuxer_iterate(void **opaque);
extern const AVOutputFormat *av_muxer_iterate(void **opaque);
extern const char *avio_enum_protocols(void **opaque, int output);
extern const char *avcodec_configuration(void);
extern const char *av_version_info(void);
extern int av_codec_is_decoder(const AVCodec *codec);
extern int av_codec_is_encoder(const AVCodec *codec);
/* Faithful declarations, checked against the pinned sources:
 *   ffmpeg-6.0/libavfilter/avfilter.h  avfilter_filter_pad_count -> unsigned
 *                                     avfilter_pad_get_type      -> enum AVMediaType
 *   mpv-0.36.0/libmpv/client.h         mpv_client_api_version    -> unsigned long
 * Wrong int/printf("%d") declarations were UB on the real ABI. */
extern unsigned avfilter_filter_pad_count(const AVFilter *filter, int is_output);
extern enum AVMediaType avfilter_pad_get_type(const AVFilterPad *pads, int pad_idx);
extern unsigned long mpv_client_api_version(void);

static int print_pad_types(const AVFilter *filter, int is_output) {
    const unsigned count = avfilter_filter_pad_count(filter, is_output);
    if (count == 0) {
        printf("-");
        return 0;
    }
    for (unsigned index = 0; index < count; index++) {
        if (index > 0) printf(",");
        /* Cast the enum to int so the TSV keeps the plain signed integers the
         * checker's parsePadTypes() expects (e.g. -1 for UNKNOWN). */
        printf("%d", (int)avfilter_pad_get_type(is_output ? filter->outputs : filter->inputs, (int)index));
    }
    return 0;
}

static void print_loaded(const char *symbol, const void *address) {
    Dl_info info;
    if (dladdr(address, &info) != 0 && info.dli_fname != NULL) {
        printf("loaded\t%s\t%s\n", symbol, info.dli_fname);
    } else {
        printf("loaded\t%s\t?\n", symbol);
    }
}

int main(void) {
    void *opaque = NULL;
    const AVFilter *filter;
    const AVCodec *codec;
    const AVInputFormat *format;
    const AVOutputFormat *outputFormat;
    const char *protocol;
    size_t filters = 0;
    size_t decoders = 0;
    size_t encoders = 0;
    size_t demuxers = 0;
    size_t muxers = 0;
    size_t protocolsIn = 0;
    size_t protocolsOut = 0;
    int padFailure = 0;

    printf("version\t%s\n", av_version_info());
    printf("config\t%s\n", avcodec_configuration());
    /* unsigned long; a single decimal integer that Number() can parse. */
    printf("mpv\t%lu\n", mpv_client_api_version());

    /* dladdr reports the image the loader actually mapped. The checker
     * requires every one of these paths to live under the vendor directory. */
    print_loaded("avcodec_configuration", (const void *)avcodec_configuration);
    print_loaded("avfilter_filter_pad_count", (const void *)avfilter_filter_pad_count);
    print_loaded("av_demuxer_iterate", (const void *)av_demuxer_iterate);
    print_loaded("mpv_client_api_version", (const void *)mpv_client_api_version);

    while ((filter = av_filter_iterate(&opaque)) != NULL) {
        printf("filter\t%s\t", filter->name);
        if (print_pad_types(filter, 0) != 0) padFailure = 1;
        printf("\t");
        if (print_pad_types(filter, 1) != 0) padFailure = 1;
        printf("\n");
        filters++;
    }

    opaque = NULL;
    while ((codec = av_codec_iterate(&opaque)) != NULL) {
        /* av_codec_iterate returns encoders and decoders alike. Splitting them
         * is the point: a minimal audio build must register decoders but no
         * encoders at all. */
        if (av_codec_is_decoder(codec)) {
            printf("decoder\t%s\t%d\n", codec->name, codec->type);
            decoders++;
        } else if (av_codec_is_encoder(codec)) {
            printf("encoder\t%s\t%d\n", codec->name, codec->type);
            encoders++;
        }
    }

    opaque = NULL;
    while ((format = av_demuxer_iterate(&opaque)) != NULL) {
        printf("demuxer\t%s\n", format->name);
        demuxers++;
    }

    opaque = NULL;
    while ((outputFormat = av_muxer_iterate(&opaque)) != NULL) {
        printf("muxer\t%s\n", outputFormat->name);
        muxers++;
    }

    opaque = NULL;
    while ((protocol = avio_enum_protocols(&opaque, 0)) != NULL) {
        printf("protocol_in\t%s\n", protocol);
        protocolsIn++;
    }
    opaque = NULL;
    while ((protocol = avio_enum_protocols(&opaque, 1)) != NULL) {
        printf("protocol_out\t%s\n", protocol);
        protocolsOut++;
    }

    printf(
        "counts\tfilters=%zu decoders=%zu encoders=%zu demuxers=%zu muxers=%zu protocols_in=%zu protocols_out=%zu\n",
        filters,
        decoders,
        encoders,
        demuxers,
        muxers,
        protocolsIn,
        protocolsOut);

    fprintf(
        stderr,
        "probe counts: filter=%zu decoder=%zu encoder=%zu demuxer=%zu muxer=%zu protocol_in=%zu protocol_out=%zu\n",
        filters,
        decoders,
        encoders,
        demuxers,
        muxers,
        protocolsIn,
        protocolsOut);
    if (filters == 0 || decoders == 0 || demuxers == 0 || protocolsIn == 0) {
        fprintf(stderr, "probe enumerated an empty core component set\n");
        return 1;
    }
    if (padFailure) {
        fprintf(stderr, "probe could not read a filter pad count\n");
        return 1;
    }
    return 0;
}
