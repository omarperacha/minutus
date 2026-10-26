# Bundled speech runtime

Bootstrap builds the helper from the pinned whisper.cpp source archive. The
helper accepts the bounded ASR pipe protocol and host-verified model paths; it
has no microphone, network or application-storage interface. Model weights are
downloaded separately. Full MIT notices are retained in this directory.

`speech-runtime/runtime.lock` binds the actual exported build inputs. The public
fixture path differs from internal development, so its build-script digest is
updated to the exact exported bytes without changing the upstream source pin.
