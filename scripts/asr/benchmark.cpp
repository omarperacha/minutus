// Offline qualification probe, not the production ASR helper or its protocol.
#include "whisper.h"

#include <chrono>
#include <fstream>
#include <iomanip>
#include <iostream>
#include <sstream>
#include <stdexcept>
#include <string>
#include <vector>
#ifdef __APPLE__
#include <libproc.h>
#include <sys/resource.h>
#include <unistd.h>
#endif

using Clock = std::chrono::steady_clock;

static double elapsed_ms(Clock::time_point start) {
    return std::chrono::duration<double, std::milli>(Clock::now() - start).count();
}

static std::string json_string(const std::string & text) {
    std::ostringstream out;
    out << '"';
    for (unsigned char c : text) {
        if (c == '"' || c == '\\') out << '\\' << c;
        else if (c < 32) out << "\\u" << std::hex << std::setw(4)
                             << std::setfill('0') << static_cast<int>(c);
        else out << c;
    }
    out << '"';
    return out.str();
}

static void print_memory() {
#ifdef __APPLE__
    rusage_info_v4 usage{};
    if (proc_pid_rusage(getpid(), RUSAGE_INFO_V4,
            reinterpret_cast<rusage_info_t *>(&usage)) != 0)
        throw std::runtime_error("cannot read process physical footprint");
    std::cout << ",\"physical_footprint_bytes\":" << usage.ri_phys_footprint
              << ",\"peak_physical_footprint_bytes\":" << usage.ri_lifetime_max_phys_footprint;
#endif
}

static std::vector<float> read_pcm(const std::string & path) {
    std::ifstream in(path, std::ios::binary | std::ios::ate);
    const auto length = in.tellg();
    if (!in || length <= 0 || length > 16 * 1024 * 1024 || length % 4 != 0)
        throw std::runtime_error("invalid bounded f32 PCM file");
    std::vector<float> samples(static_cast<size_t>(length) / sizeof(float));
    in.seekg(0);
    if (!in.read(reinterpret_cast<char *>(samples.data()), length))
        throw std::runtime_error("incomplete PCM read");
    return samples;
}

int main(int argc, char ** argv) {
    try {
        if (argc != 4) throw std::runtime_error("usage: asr-benchmark MODEL cpu|metal CASES.tsv");
        const std::string backend = argv[2];
        if (backend != "cpu" && backend != "metal") throw std::runtime_error("unknown backend");
        std::ifstream cases(argv[3]);
        if (!cases) throw std::runtime_error("cannot open cases");
        auto config = whisper_context_default_params();
        config.use_gpu = backend == "metal";
        const auto load_start = Clock::now();
        auto * ctx = whisper_init_from_file_with_params(argv[1], config);
        if (!ctx) throw std::runtime_error("model load failed");
        std::cout << "{\"type\":\"loaded\",\"load_ms\":" << elapsed_ms(load_start);
        print_memory();
        std::cout << "}" << std::endl;
        std::string line;
        int index = 0;
        while (std::getline(cases, line)) {
            std::istringstream row(line);
            std::string id, language, path;
            if (!std::getline(row, id, '\t') || !std::getline(row, language, '\t')
                || !std::getline(row, path) || (language != "auto"
                    && whisper_lang_id(language.c_str()) < 0)) throw std::runtime_error("invalid case");
            auto samples = read_pcm(path);
            auto params = whisper_full_default_params(WHISPER_SAMPLING_GREEDY);
            params.n_threads = 6;
            params.language = language.c_str();
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
            const auto start = Clock::now();
            if (whisper_full(ctx, params, samples.data(), static_cast<int>(samples.size())) != 0)
                throw std::runtime_error("recognition failed");
            const double ms = elapsed_ms(start);
            std::string text;
            for (int i = 0; i < whisper_full_n_segments(ctx); ++i)
                text += whisper_full_get_segment_text(ctx, i);
            std::cout << "{\"type\":\"result\",\"index\":" << index++
                      << ",\"id\":" << json_string(id)
                      << ",\"requested_language\":" << json_string(language)
                      << ",\"detected_language\":" << json_string(whisper_lang_str(whisper_full_lang_id(ctx)))
                      << ",\"audio_seconds\":" << samples.size() / 16000.0
                      << ",\"inference_ms\":" << ms << ",\"text\":" << json_string(text);
            print_memory();
            std::cout << "}" << std::endl;
        }
        whisper_free(ctx);
        return 0;
    } catch (const std::exception & error) {
        std::cerr << error.what() << '\n';
        return 1;
    }
}
