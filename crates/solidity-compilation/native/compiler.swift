// SPDX-License-Identifier: GPL-3.0-only
// This standalone compiler process is distributed with its complete source.
// Source is data, never JavaScript. Only an official checksum-verified compiler
// program is evaluated in this separate sandboxed JavaScriptCore process.
import Foundation
import JavaScriptCore
import Darwin
import Dispatch

struct Request: Decodable {
    let compiler: String
    let version: String
    let input: String
}
struct Response: Encodable {
    let version: String
    let output: String
}
func fail() -> Never { exit(1) }
let inputLimit = 64 * 1024 * 1024
var cpu = rlimit(rlim_cur: 45, rlim_max: 45)
var core = rlimit(rlim_cur: 0, rlim_max: 0)
// FD3 is the broker-owned lifetime pipe. It is mandatory before any input;
// broker death/cancellation closes its sole writer and kills this worker.
var lifetimeInfo = stat()
let lifetimeFlags = fcntl(3, F_GETFL)
guard fstat(3, &lifetimeInfo) == 0,
      (lifetimeInfo.st_mode & S_IFMT) == S_IFIFO,
      lifetimeFlags >= 0,
      (lifetimeFlags & O_ACCMODE) == O_RDONLY,
      (lifetimeFlags & O_NONBLOCK) == 0,
      fcntl(3, F_SETFD, FD_CLOEXEC) == 0 else { fail() }
// No other inherited application descriptors survive single-threaded startup.
for descriptor in 4..<getdtablesize() { close(descriptor) }
var descriptors = rlimit(rlim_cur: 64, rlim_max: 64)
guard setrlimit(RLIMIT_CPU, &cpu) == 0,
      setrlimit(RLIMIT_CORE, &core) == 0,
      setrlimit(RLIMIT_NOFILE, &descriptors) == 0 else { fail() }
DispatchQueue.global(qos: .userInitiated).async {
    var byte: UInt8 = 0
    while true {
        let count = read(3, &byte, 1)
        if count < 0 && errno == EINTR { continue }
        _exit(70) // EOF, unexpected data and every read failure are terminal.
    }
}
do {
    var input = Data()
    while let chunk = try FileHandle.standardInput.read(upToCount: 64 * 1024), !chunk.isEmpty {
        guard input.count + chunk.count <= inputLimit else { fail() }
        input.append(chunk)
    }
    let request = try JSONDecoder().decode(Request.self, from: input)
    guard request.compiler.utf8.count <= 40 * 1024 * 1024,
          request.input.utf8.count <= 4 * 1024 * 1024,
          request.version.utf8.count <= 100,
          let context = JSContext() else { fail() }
    var failed = false
    context.exceptionHandler = { _, _ in failed = true }
    context.evaluateScript("var Module = {print: function(){}, printErr: function(){}};")
    context.evaluateScript(request.compiler)
    guard !failed,
          let version = context.evaluateScript("Module.cwrap('solidity_version','string',[])()")?.toString(),
          version == request.version + ".Emscripten.clang" else { fail() }
    context.setObject(request.input, forKeyedSubscript: "inputJSON" as NSString)
    guard let output = context.evaluateScript("Module.cwrap('solidity_compile','string',['string','number','number'])(inputJSON,0,0)")?.toString(),
          !failed, output.utf8.count <= 16 * 1024 * 1024 else { fail() }
    let encoded = try JSONEncoder().encode(Response(version: version, output: output))
    guard encoded.count <= 32 * 1024 * 1024 else { fail() }
    try FileHandle.standardOutput.write(contentsOf: encoded)
} catch { fail() }
