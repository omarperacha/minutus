# Using Minutus

## Models

On first launch, choose a chat model and download its bundle or import matching
local files. Installed models and the default for new projects/chats are managed in **Minutus settings**.
Projects can select a different installed model in their own settings.
The [README compatibility table](../README.md#model-compatibility) lists sizes,
capabilities and memory recommendations. Larger contexts need more memory.

Choose **Import local files** to expand the module list. Every module is labelled
**Required** or **Optional**, including after it is ready. Each module has its own
**Import** and **Download** actions. The file picker names that specific module and
its full expected filename; choosing a file starts its SHA-256 verification
immediately. Cancelling the picker leaves all previously verified modules intact.
A similarly named file or different quantisation is not enough.

You can mix sources: import the main weights and download just the missing
projector, for example. The module list shows **Imported · Verified** or
**Downloaded · Verified** after each successful operation. Verification of a
large local file can take time without a download.

Both Qwen and Glimmer require their main GGUF and image projector.
Glimmer’s faster generation draft is optional. **Download required modules**
downloads any missing required files; **Download all modules** includes optional
files too. Already verified files are retained. The main button becomes
**Finish setup** once every required module is verified; optional modules can be
added later in Settings: choose **Manage modules** on the installed model
card to open the module list. Setup completes automatically
when every module is ready. Image projectors remain unloaded until image understanding needs them.

External files remain yours: Minutus does not copy, move or delete them.
**Remove from Minutus** in Settings forgets their registration and app-owned runtime references.
Removing a downloaded, app-managed model is a separate operation that can delete
those managed weights.

| What you see | What to do |
| --- | --- |
| Not installed | Download the model or import the matching local files. |
| Setup incomplete | Import or download each missing **Required** module. Already verified modules are retained. |
| Unavailable | Reconnect its drive or restore access, then use **Locate local files** in the expanded import controls to verify the files at their current locations. |
| Verification failed | Supply unchanged catalogue files; renaming an incompatible file will not make it valid. |
| Download paused/failed | Retry to resume retained partial data. A cancelled download does not register an incomplete model. |

On macOS, new imports retain read-only file access across app launches. If you
imported with an older build that stored only a path, use **Import local files**,
then **Locate local files** for each unavailable module once. Verification saves
the persistent access grant without copying or moving the weights. Files on a
disconnected drive, or access revoked by macOS, can still require locating again.

## Projects and files

Create a project to group related chats. Chat works without granting a folder.
Add a folder in project settings when you want file tools. Choose a specific
project directory: home, Library or another folder containing Minutus's private
application data is rejected. Credential and execution-control paths are also
protected, even inside an otherwise granted folder.

New grants are read-only. Enabling read and write permits ordinary edits without
a confirmation for each one. Review the diffs and use undo when needed. A writable
folder may contain runnable code; review changes before executing it elsewhere.
After changing folder permissions, you can retry in the same chat. Minutus can
reuse previously discovered file references, subject to current access checks.
See [security](security.md) for the exact boundary.

## Optional features

- **Web search:** save a Brave Search API key in **Minutus settings**. Queries go
  directly to Brave after native approval; opening a result needs separate URL
  approval. A browser account or a wallet is not needed for local chat.
- **Dictation:** download the separate speech model in settings, then use the
  composer microphone and grant macOS microphone permission. Transcription stays
  local. Dictation and chat generation share one foreground slot.
- **Appearance:** Light is the default. Choose Light, Dark or System in settings;
  the choice is saved for the next launch.

## Wallet

The optional wallet works in source builds without Apple developer credentials.
Its app password must be at least 15 characters. Each newly generated account has
its own 24-word recovery phrase; an imported private key needs its own backup.
One account's phrase does not recover other independent accounts.

Unlock to verify accounts and display receiving addresses. Native password and
approval windows handle secrets and signing. Keep phrases and keys offline;
never paste them into chat, an issue or an email.

If you forget the app password, the locked wallet's security settings offer
recovery by phrase or private key. **This replaces the entire app vault with one
recovered account.** Other accounts require their own backups. Removing an account
or deleting the vault does not recover it or securely erase backups. There is no
silent migration from older wallet storage. See [security](security.md#wallet-recovery-and-transaction-approvals).

### EVM capabilities by entry point

| Capability | Wallet screen | Chat |
| --- | --- | --- |
| ETH and native USDC transfers on Base and Ethereum mainnet | Available, with native review and fresh password authentication | Not available |
| ETH and native USDC transfers on Base Sepolia and Ethereum Sepolia | Available for testing, with the same approval requirements | Not available |
| x402 exact USDC payments on Base mainnet | Initiated through chat | Available for supported public HTTPS services; separate request and payment approval |
| Arbitrary contract calls, swaps and token approvals | Not available | Not available |
| Arbitrary typed-data or message signing | Not available | Not available |

Choose the network explicitly in the Wallet transfer screen. ETH pays network
fees; token transfers require ETH as well as the token balance. The wallet's
transaction/authorisation decoder inspects data; it does not provide a general
signing or contract-execution interface. Chat cannot invoke the Wallet screen's
transfer commands. The debug-only local x402 qualification tool uses Base Sepolia;
it does not enable general testnet transactions from chat.

## Paid x402 resources

With an optional wallet, Minutus can use supported public HTTPS x402 services
with USDC on Base mainnet. Review the destination and any request body before
they leave your Mac, then separately review the exact payment and authenticate
with the app password. Discovery alone never authorises spending. If a payment
outcome is uncertain, check the existing receipt or merchant order rather than
asking Minutus to repeat the purchase. See [supported services and limits](x402.md).

## Help and bug reports

If native helpers are missing during a source build, rerun bootstrap as described
in [build troubleshooting](build.md#run-the-release-build). An unavailable model
is handled separately using the table above.

For a bug report, include the source commit or app version, macOS version, Mac
chip and unified memory, model/profile, steps to reproduce and a redacted error.
Use synthetic inputs where possible. Keep raw traces, workspace content and
wallet material private. Contact [hello@minutus.ai](mailto:hello@minutus.ai);
report vulnerabilities privately through [SECURITY.md](../SECURITY.md).
