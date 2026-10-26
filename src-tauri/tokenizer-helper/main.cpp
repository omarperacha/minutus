#include "llama.h"

#include <cstdint>
#include <iostream>
#include <limits>
#include <string>
#include <vector>

namespace {
constexpr std::uint64_t kMaxPromptBytes = 64ULL * 1024ULL * 1024ULL;

bool read_exact(char * data, std::size_t size) {
    std::cin.read(data, static_cast<std::streamsize>(size));
    return static_cast<std::size_t>(std::cin.gcount()) == size;
}

bool read_u64(std::uint64_t & value) {
    unsigned char bytes[8]{};
    if (!read_exact(reinterpret_cast<char *>(bytes), sizeof(bytes))) {
        return false;
    }
    value = 0;
    for (unsigned int index = 0; index < 8; ++index) {
        value |= static_cast<std::uint64_t>(bytes[index]) << (index * 8);
    }
    return true;
}

void write_u64(std::uint64_t value) {
    unsigned char bytes[8]{};
    for (unsigned int index = 0; index < 8; ++index) {
        bytes[index] = static_cast<unsigned char>((value >> (index * 8)) & 0xffU);
    }
    std::cout.write(reinterpret_cast<const char *>(bytes), sizeof(bytes));
    std::cout.flush();
}
} // namespace

int main(int argc, char ** argv) {
    if (argc != 2) {
        return 2;
    }

    llama_backend_init();
    llama_model_params params = llama_model_default_params();
    params.vocab_only = true;
    params.n_gpu_layers = 0;

    llama_model * model = llama_model_load_from_file(argv[1], params);
    if (model == nullptr) {
        return 3;
    }
    const llama_vocab * vocab = llama_model_get_vocab(model);
    if (vocab == nullptr) {
        llama_model_free(model);
        return 4;
    }

    for (;;) {
        std::uint64_t byte_count = 0;
        if (!read_u64(byte_count)) {
            break;
        }
        if (byte_count == 0 || byte_count > kMaxPromptBytes ||
            byte_count > static_cast<std::uint64_t>(std::numeric_limits<std::size_t>::max())) {
            write_u64(std::numeric_limits<std::uint64_t>::max());
            continue;
        }

        std::string prompt(static_cast<std::size_t>(byte_count), '\0');
        if (!read_exact(prompt.data(), prompt.size())) {
            llama_model_free(model);
            return 5;
        }
        const int tokenized = llama_tokenize(
            vocab, prompt.data(), static_cast<int32_t>(prompt.size()), nullptr, 0,
            false, true);
        if (tokenized >= 0) {
            write_u64(static_cast<std::uint64_t>(tokenized));
            continue;
        }
        const int required = -tokenized;
        if (required <= 0) {
            write_u64(std::numeric_limits<std::uint64_t>::max());
            continue;
        }
        std::vector<llama_token> tokens(static_cast<std::size_t>(required));
        const int actual = llama_tokenize(
            vocab, prompt.data(), static_cast<int32_t>(prompt.size()), tokens.data(),
            static_cast<int32_t>(tokens.size()), false, true);
        write_u64(actual < 0 ? std::numeric_limits<std::uint64_t>::max()
                             : static_cast<std::uint64_t>(actual));
    }

    llama_model_free(model);
    llama_backend_free();
    return 0;
}
