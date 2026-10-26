// Private, bounded Whisper process used by model-runtime. Not a user-facing CLI.
#include "whisper.h"

#include <algorithm>
#include <array>
#include <cmath>
#include <cstdint>
#include <cstring>
#include <iostream>
#include <memory>
#include <stdexcept>
#include <string>
#include <vector>

namespace {
constexpr std::array<unsigned char, 4> kMagic{'Z', 'A', 'S', 'R'};
constexpr std::uint16_t kProtocolVersion = 1;
constexpr std::uint8_t kRequest = 1;
constexpr std::uint8_t kFinal = 1;
constexpr std::uint8_t kNoSpeech = 2;
constexpr std::uint8_t kFailure = 3;
constexpr std::uint64_t kMaxSamples = 120ULL * 16000ULL;
constexpr std::size_t kMaxLanguageBytes = 8;
constexpr std::size_t kMaxTranscriptBytes = 65535;

struct Request {
    bool use_gpu;
    std::string language;
    std::vector<float> samples;
};

void read_exact(void *destination, std::size_t length) {
    if (!std::cin.read(static_cast<char *>(destination), static_cast<std::streamsize>(length))) {
        throw std::runtime_error("incomplete request");
    }
}

std::uint16_t little_u16(const unsigned char *bytes) {
    return static_cast<std::uint16_t>(bytes[0]) |
           (static_cast<std::uint16_t>(bytes[1]) << 8U);
}

std::uint64_t little_u64(const unsigned char *bytes) {
    std::uint64_t value = 0;
    for (unsigned int index = 0; index < 8; ++index) {
        value |= static_cast<std::uint64_t>(bytes[index]) << (index * 8U);
    }
    return value;
}

Request read_request() {
    std::array<unsigned char, 18> fixed{};
    read_exact(fixed.data(), fixed.size());
    if (!std::equal(kMagic.begin(), kMagic.end(), fixed.begin()) ||
        little_u16(fixed.data() + 4) != kProtocolVersion || fixed[6] != kRequest ||
        fixed[7] > 1) {
        throw std::runtime_error("protocol mismatch");
    }
    const auto sample_count = little_u64(fixed.data() + 8);
    const auto language_length = little_u16(fixed.data() + 16);
    if (sample_count == 0 || sample_count > kMaxSamples ||
        language_length > kMaxLanguageBytes) {
        throw std::runtime_error("request exceeds bounds");
    }
    Request request{fixed[7] == 0, std::string(language_length, '\0'),
                    std::vector<float>(static_cast<std::size_t>(sample_count))};
    read_exact(request.language.data(), request.language.size());
    std::array<unsigned char, 4> encoded{};
    for (auto &sample : request.samples) {
        read_exact(encoded.data(), encoded.size());
        const std::uint32_t bits = static_cast<std::uint32_t>(encoded[0]) |
                                   (static_cast<std::uint32_t>(encoded[1]) << 8U) |
                                   (static_cast<std::uint32_t>(encoded[2]) << 16U) |
                                   (static_cast<std::uint32_t>(encoded[3]) << 24U);
        std::memcpy(&sample, &bits, sizeof(sample));
        if (!std::isfinite(sample) || sample < -1.0F || sample > 1.0F) {
            throw std::runtime_error("invalid sample");
        }
    }
    if (std::cin.get() != std::char_traits<char>::eof()) {
        throw std::runtime_error("trailing request bytes");
    }
    if (!request.language.empty() && whisper_lang_id(request.language.c_str()) < 0) {
        throw std::runtime_error("unsupported language");
    }
    return request;
}

void write_u16(std::uint16_t value) {
    const std::array<unsigned char, 2> bytes{static_cast<unsigned char>(value),
                                             static_cast<unsigned char>(value >> 8U)};
    std::cout.write(reinterpret_cast<const char *>(bytes.data()), bytes.size());
}

void write_u64(std::uint64_t value) {
    std::array<unsigned char, 8> bytes{};
    for (unsigned int index = 0; index < 8; ++index) {
        bytes[index] = static_cast<unsigned char>(value >> (index * 8U));
    }
    std::cout.write(reinterpret_cast<const char *>(bytes.data()), bytes.size());
}

void write_response(std::uint8_t kind, const std::string &language, const std::string &text) {
    if (language.size() > kMaxLanguageBytes || text.size() > kMaxTranscriptBytes) {
        throw std::runtime_error("result exceeds bounds");
    }
    std::cout.write(reinterpret_cast<const char *>(kMagic.data()), kMagic.size());
    write_u16(kProtocolVersion);
    std::cout.put(static_cast<char>(kind));
    std::cout.put('\0');
    write_u64(1);
    write_u16(static_cast<std::uint16_t>(language.size()));
    write_u16(static_cast<std::uint16_t>(text.size()));
    std::cout.write(language.data(), static_cast<std::streamsize>(language.size()));
    std::cout.write(text.data(), static_cast<std::streamsize>(text.size()));
    std::cout.flush();
    if (!std::cout) throw std::runtime_error("result write failed");
}

std::string trim(std::string text) {
    const auto first = text.find_first_not_of(" \t\r\n");
    if (first == std::string::npos) return {};
    const auto last = text.find_last_not_of(" \t\r\n");
    return text.substr(first, last - first + 1);
}
}  // namespace

int main(int argc, char **argv) {
    try {
        if (argc != 5 || std::string(argv[1]) != "--model" ||
            std::string(argv[3]) != "--threads") {
            throw std::runtime_error("invalid invocation");
        }
        const int threads = std::stoi(argv[4]);
        if (threads < 1 || threads > 64) throw std::runtime_error("invalid worker count");
        const Request request = read_request();
        auto context_params = whisper_context_default_params();
        context_params.use_gpu = request.use_gpu;
        using Context = std::unique_ptr<whisper_context, decltype(&whisper_free)>;
        Context context(whisper_init_from_file_with_params(argv[2], context_params), whisper_free);
        if (!context) throw std::runtime_error("model load failed");

        auto params = whisper_full_default_params(WHISPER_SAMPLING_GREEDY);
        params.n_threads = threads;
        params.language = request.language.empty() ? "auto" : request.language.c_str();
        params.translate = false;
        params.no_context = true;
        params.no_timestamps = true;
        params.print_realtime = false;
        params.print_progress = false;
        params.print_timestamps = false;
        params.print_special = false;
        params.temperature = 0;
        params.temperature_inc = 0;
        params.greedy.best_of = 1;
        if (whisper_full(context.get(), params, request.samples.data(),
                         static_cast<int>(request.samples.size())) != 0) {
            throw std::runtime_error("recognition failed");
        }
        std::string text;
        for (int index = 0; index < whisper_full_n_segments(context.get()); ++index) {
            text += whisper_full_get_segment_text(context.get(), index);
            if (text.size() > kMaxTranscriptBytes) throw std::runtime_error("result exceeds bounds");
        }
        text = trim(std::move(text));
        if (text.empty()) {
            write_response(kNoSpeech, {}, {});
        } else {
            const char *detected = whisper_lang_str(whisper_full_lang_id(context.get()));
            if (detected == nullptr) throw std::runtime_error("language unavailable");
            write_response(kFinal, detected, text);
        }
        return 0;
    } catch (const std::exception &) {
        try {
            write_response(kFailure, {}, {});
        } catch (const std::exception &) {
        }
        return 1;
    }
}
