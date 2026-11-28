//! The `lexi` command tree (clap).
//!
//! Grammar follows the Python CLI (`docs/cli-reference.md`): plural nouns with
//! verb subcommands (`prompts list`, `models alias set`, …). Commands are added
//! slice by slice as they become fully functional — see
//! `docs/cli-consolidation-plan.md` and `docs/surface-matrix.md`.

use std::path::PathBuf;

use clap::{Parser, Subcommand};

#[derive(Debug, Parser)]
#[command(
    name = "lexi",
    version,
    about = "Lexi — work with multiple LLM providers side by side",
    propagate_version = true,
    disable_help_subcommand = true,
    after_help = "Run `lexi` with no command for the interactive shell. \
                  Topics: `lexi help general | ai | shortcuts`."
)]
pub struct Cli {
    /// Machine-readable output: one JSON value per result (NDJSON events when
    /// streaming); errors as {"error": ...} on stderr.
    #[arg(long, global = true)]
    pub json: bool,

    /// The command; none starts the interactive shell.
    #[command(subcommand)]
    pub cmd: Option<Cmd>,
}

#[derive(Debug, Subcommand)]
pub enum Cmd {
    /// Send a prompt to a provider; get back text, images, or both.
    ///
    /// respond <provider> [TEXT…|-] [--prompt NAME] [--model M] [--output KIND]
    /// [--out DIR] [--role R] [--stream] [--background [--detach]]
    /// [--image URL|PATH]... [<schema flags>] [--set path=value]...
    /// [--unset path]... [--tool preset[,preset]]... [--params FILE] [--cite]
    /// [--raw|-r] [--verbose|-v] [--no-validate] [--timeout SECS] [--key KEY]
    ///
    /// The words are the prompt (`-` reads stdin); with none, the saved prompt
    /// --prompt NAME, else the active one. The model's tags pick the API;
    /// --output image picks the provider's image API and default image model.
    /// Images are saved to --out (default: here), never overwriting a file.
    ///
    /// Schema flags (e.g. --temperature, --size) come from the selected API's
    /// definition: see `lexi api show <api>` (the cli= entries).
    ///
    /// --timeout SECS sets this request's time limit (0 = none) instead of
    /// request_timeout in config.json: the whole wait, or with --stream the
    /// longest silence.
    ///
    /// --key KEY (or PROVIDER#KEY) uses one of the provider's named keys
    /// (`providers keys list`): another account's.
    Respond {
        /// Provider name, as in providers-config.yaml (PROVIDER#KEY for a named key).
        provider: String,
        /// Prompt name and options (see above).
        #[arg(
            trailing_var_arg = true,
            allow_hyphen_values = true,
            value_name = "ARGS"
        )]
        args: Vec<String>,
    },
    /// Generate image(s) and save them: shortcut for `respond <provider> TEXT --output image`.
    Generate {
        /// Provider name.
        provider: String,
        /// The image prompt.
        prompt: String,
        /// Override the provider's default model.
        #[arg(long)]
        model: Option<String>,
        /// Directory to write images into.
        #[arg(long, default_value = ".")]
        out: PathBuf,
        /// This request's time limit in seconds (0 = none), instead of
        /// request_timeout in config.json.
        #[arg(long, value_name = "SECS")]
        timeout: Option<u64>,
        /// Use this named key of the provider (or write PROVIDER#KEY).
        #[arg(long, value_name = "KEY")]
        key: Option<String>,
    },
    /// Embed texts as vectors (one per text) with a provider's embedding model.
    ///
    /// embed <provider> TEXT... [--file FILE] [-] [--model M] [<schema flags>]
    /// [--set path=value]... [--unset path]... [--no-validate] [--timeout SECS]
    /// [--key KEY]
    ///
    /// Each argument is one text; --file adds one text per line, `-` reads
    /// lines from stdin. Schema flags (e.g. --dimensions, --input-type) come
    /// from the embedding API's definition. Use --json for the vectors.
    /// --timeout SECS sets this request's time limit (0 = none).
    #[command(verbatim_doc_comment)]
    Embed {
        /// Provider name, as in providers-config.yaml.
        provider: String,
        /// Texts and options (see above).
        #[arg(
            trailing_var_arg = true,
            allow_hyphen_values = true,
            value_name = "ARGS"
        )]
        args: Vec<String>,
    },
    /// Upload a file to a provider's Files API; prints the file id.
    Upload {
        /// Provider name.
        provider: String,
        /// Path to the file to upload.
        file: PathBuf,
        /// Upload with this named key of the provider (or write
        /// PROVIDER#KEY); a file belongs to that key's account.
        #[arg(long, value_name = "KEY")]
        key: Option<String>,
    },
    /// Check or wait for a background job (`respond --background --detach`).
    Jobs {
        #[command(subcommand)]
        cmd: JobsCmd,
    },
    /// Show the model tag vocabulary (namespaces, kinds and what they imply).
    Tags,
    /// Show the CLI version.
    Version,
    /// Show help for a command (`help models alias`) or a topic
    /// (`general`, `ai`, `shortcuts`).
    Help { topic: Vec<String> },
    /// Manage command aliases (`aliases.json`).
    Alias {
        #[command(subcommand)]
        cmd: Option<AliasCmd>,
    },
    /// Show or clear command history.
    History {
        /// Show the last N entries (negative: the first N).
        #[arg(short = 'n', long = "number", allow_negative_numbers = true)]
        count: Option<i64>,
        /// Clear the history.
        #[arg(long, visible_alias = "reset")]
        clear: bool,
    },
    /// Run a shell command (also `!command` in the interactive shell).
    Sh {
        #[arg(trailing_var_arg = true, allow_hyphen_values = true, required = true)]
        command: Vec<String>,
    },
    /// Leave the interactive shell.
    #[command(visible_alias = "quit")]
    Exit,
    /// View or update CLI settings (config.json).
    Config {
        #[command(subcommand)]
        cmd: ConfigCmd,
    },
    /// Manage saved prompts.
    Prompts {
        #[command(subcommand)]
        cmd: PromptsCmd,
    },
    /// Manage providers.
    Providers {
        #[command(subcommand)]
        cmd: ProvidersCmd,
    },
    /// Manage a provider's models.
    Models {
        #[command(subcommand)]
        cmd: ModelsCmd,
    },
    /// Manage API definitions.
    Api {
        #[command(subcommand)]
        cmd: ApiCmd,
    },
}

#[derive(Debug, Subcommand)]
pub enum JobsCmd {
    /// Check a job once; prints its result if it has finished.
    Status {
        provider: String,
        id: String,
        /// The named key the job was started with (or write PROVIDER#KEY).
        #[arg(long, value_name = "KEY")]
        key: Option<String>,
    },
    /// Wait for a job to finish and print its result.
    Wait {
        provider: String,
        id: String,
        /// Seconds between polls.
        #[arg(long, default_value_t = 2)]
        interval: u64,
        /// Give up after this many seconds.
        #[arg(long, default_value_t = 600)]
        timeout: u64,
        /// The named key the job was started with (or write PROVIDER#KEY).
        #[arg(long, value_name = "KEY")]
        key: Option<String>,
    },
}

#[derive(Debug, Subcommand)]
pub enum AliasCmd {
    /// List aliases (optionally only those for the given commands/aliases).
    #[command(visible_alias = "ls")]
    List { names: Vec<String> },
    /// Create or update an alias: `alias add pl providers list`.
    #[command(visible_alias = "set")]
    Add {
        name: String,
        #[arg(required = true, trailing_var_arg = true, allow_hyphen_values = true)]
        expansion: Vec<String>,
    },
    /// Remove an alias.
    #[command(visible_aliases = ["del", "delete"])]
    Rm { name: String },
    /// Reset aliases to the shipped defaults.
    Reset,
}

#[derive(Debug, Subcommand)]
pub enum ConfigCmd {
    /// Show config values (all, or a single key).
    #[command(visible_alias = "ls")]
    List {
        /// Show only this key.
        key: Option<String>,
        /// Print the raw config file.
        #[arg(short, long)]
        raw: bool,
        /// With a key: print only its value.
        #[arg(short, long)]
        value: bool,
    },
    /// Set a config value (the rest of the line is the value).
    Set {
        key: String,
        #[arg(required = true, num_args = 1..)]
        value: Vec<String>,
    },
    /// Remove a config value.
    #[command(visible_aliases = ["del", "delete"])]
    Rm { key: String },
}

#[derive(Debug, Subcommand)]
pub enum PromptsCmd {
    /// List prompts (or show one). The active prompt is marked `*`.
    #[command(visible_alias = "ls")]
    List {
        /// Show only this prompt.
        name: Option<String>,
        /// Print the prompt JSON.
        #[arg(short, long)]
        raw: bool,
        /// Include max_tokens / temperature / role.
        #[arg(short, long)]
        detailed: bool,
        /// Render as a table.
        #[arg(short, long)]
        table: bool,
    },
    /// Save or update a prompt: `prompts set [name] [text…] [--prompt TEXT]
    /// [-m|--max_tokens N] [-t|--temperature T] [-r|--role ROLE]
    /// [--set path=value] [--unset path]`. With one bare word and no name, it
    /// is the text of the active prompt. Text `\n` becomes a newline.
    #[command(visible_alias = "save")]
    Set {
        #[arg(trailing_var_arg = true, allow_hyphen_values = true)]
        args: Vec<String>,
    },
    /// Remove a saved prompt.
    #[command(visible_aliases = ["del", "delete"])]
    Rm { name: String },
    /// Make a saved prompt the active one (copies it into `$$`).
    #[command(visible_alias = "use")]
    Activate { name: String },
    /// Print an ad-hoc prompt payload (defaults + text/options) without saving.
    #[command(external_subcommand)]
    Text(Vec<String>),
}

#[derive(Debug, Subcommand)]
pub enum ProvidersCmd {
    /// List configured providers.
    #[command(visible_alias = "ls")]
    List {
        /// Print the raw providers-config.yaml.
        #[arg(short, long)]
        raw: bool,
        /// Render as a table.
        #[arg(short, long)]
        table: bool,
    },
    /// Check a provider's API keys (a live call to its models endpoint; nothing is generated or billed): with a key named (PROVIDER#KEY), that key; without, every key and the provider's variable, and what a request that names no key would use.
    Status {
        name: String,
        /// Print the result as JSON.
        #[arg(short, long)]
        raw: bool,
        /// Check this key (or write PROVIDER#KEY).
        #[arg(long, value_name = "KEY")]
        key: Option<String>,
    },
    /// Add a provider.
    Add {
        name: String,
        /// Base URL, e.g. https://api.example.com/v1.
        #[arg(long)]
        url: String,
        /// The model used when none is given.
        #[arg(long = "default_model", visible_alias = "default-model")]
        default_model: String,
        /// API key (saved in the OS key store — macOS Keychain, Windows Credential Manager, the desktop keyring — else an owner-only key file; never the config), as the key --key-name names.
        #[arg(long = "api_key", visible_alias = "api-key", requires = "key_name")]
        api_key: Option<String>,
        /// The name of the key --api-key saves (keys have names: PROVIDER#NAME).
        #[arg(
            long = "key_name",
            visible_alias = "key-name",
            value_name = "NAME",
            requires = "api_key"
        )]
        key_name: Option<String>,
        /// API definition the provider speaks.
        #[arg(long, default_value = "chat-completions")]
        api: String,
        /// Path of the provider's models endpoint.
        #[arg(
            long = "hosted_models",
            visible_alias = "hosted-models",
            default_value = "/models"
        )]
        hosted_models: String,
    },
    /// Manage a provider's API keys, each named PROVIDER#NAME (PROVIDER# is a key saved before keys had names): the key Lexi saves, and the other places it's looked for first (api_key in providers-config.yaml: environment variables, your files, commands). Commands that change a key must name it; show and source list without a name cover every key.
    #[command(name = "api-key")]
    ApiKey {
        #[command(subcommand)]
        cmd: ApiKeyCmd,
    },
    /// A provider's keys (per project or per account): list, add, rename,
    /// remove, and choose the default. A request picks one with PROVIDER#KEY
    /// (or --key KEY); one that names none uses the provider's variable when
    /// it's set, else the default key.
    Keys {
        #[command(subcommand)]
        cmd: KeysCmd,
    },
    /// Manage the APIs a provider offers (its bindings).
    Api {
        #[command(subcommand)]
        cmd: ProviderApiCmd,
    },
    /// Convert providers to the shipped templates, keeping your changes.
    ///
    /// Shows what would change; --yes applies it (backing up
    /// providers-config.yaml). Values still at the old shipped defaults are
    /// dropped (the template supplies current ones); values you changed stay.
    Upgrade {
        /// Providers to upgrade (default: all that have a shipped template).
        names: Vec<String>,
        /// The template to use (default: the provider's name).
        #[arg(long)]
        template: Option<String>,
        /// Apply the change (otherwise just show it).
        #[arg(short, long)]
        yes: bool,
    },
    /// Rename a provider.
    Rename { old: String, new: String },
    /// Remove a provider and its models, aliases and saved key.
    #[command(visible_aliases = ["del", "delete"])]
    Rm { name: String },
}

#[derive(Debug, Subcommand)]
pub enum ProviderApiCmd {
    /// List a provider's APIs: which models each serves, its default model,
    /// and whether it comes from the template or your config.
    #[command(visible_alias = "ls")]
    List { name: String },
    /// Add an API to a provider (or update one), e.g.
    /// `providers api add openai embeddings --for kind:embedding --default-model text-embedding-3-small`.
    Add {
        name: String,
        /// API definition name (see `api list`).
        api: String,
        /// Tags a model needs to use this API (repeatable); none = the catch-all.
        #[arg(long = "for", value_name = "TAG")]
        for_tags: Vec<String>,
        /// The model used when a request asks for this API's output without naming one.
        #[arg(long = "default-model", visible_alias = "default_model")]
        default_model: Option<String>,
        /// A base URL for this API, if it differs from the provider's.
        #[arg(long)]
        url: Option<String>,
    },
    /// Remove an API from a provider (a template's API is turned off).
    #[command(visible_aliases = ["del", "delete"])]
    Rm { name: String, api: String },
}

#[derive(Debug, Subcommand)]
pub enum KeysCmd {
    /// List the provider's keys with where each comes from, which is the
    /// default (★), the provider's variable, and what a request that names
    /// no key uses.
    #[command(visible_alias = "ls")]
    List { name: String },
    /// Add a key without a value (PROVIDER#KEY, or PROVIDER KEY): save its
    /// value with `providers api-key set PROVIDER#KEY` (which also adds a new
    /// key), or give it sources with `providers api-key source add
    /// PROVIDER#KEY …`.
    Add { name: String, key: Option<String> },
    /// Rename a key — PROVIDER#OLD NEW, or PROVIDER OLD NEW (PROVIDER# is a
    /// key saved before keys had names) — its sources and saved key go along,
    /// and it stays the default if it was.
    Rename {
        name: String,
        old: String,
        new: Option<String>,
    },
    /// Remove a key and its saved key (PROVIDER#KEY, or PROVIDER KEY). Any
    /// key can go: without the default, a request has to name a key until you
    /// choose another default.
    #[command(visible_aliases = ["del", "remove"])]
    Rm { name: String, key: Option<String> },
    /// Show the key requests use when they name none, or choose it
    /// (PROVIDER#KEY, or PROVIDER KEY).
    Default { name: String, key: Option<String> },
}

#[derive(Debug, Subcommand)]
pub enum ApiKeyCmd {
    /// Save a key's value as its saved key (in the OS key store — macOS Keychain, Windows Credential Manager, the Linux desktop keyring — else an owner-only file; key_store in config picks), looked in after the sources its api_key lists. Name the key: PROVIDER#KEY (a new name adds the key), PROVIDER# for one saved before keys had names. Prompted for, or read from stdin, without --value.
    Set {
        name: String,
        #[arg(short, long)]
        value: Option<String>,
        /// The key (or write PROVIDER#KEY).
        #[arg(long = "name", value_name = "KEY")]
        key: Option<String>,
    },
    /// Show where a key is looked for, in order (the saved key among them), and the key, masked unless --reveal — every key, and the provider's variable, when none is named. Commands are run only with --reveal.
    Show {
        name: String,
        #[arg(short, long)]
        reveal: bool,
        /// The key (or write PROVIDER#KEY).
        #[arg(long = "name", value_name = "KEY")]
        key: Option<String>,
    },
    /// Delete a key's saved value (PROVIDER#KEY). The key, its sources and the config stay; to remove the key itself, use `providers keys rm`.
    Remove {
        name: String,
        /// The key (or write PROVIDER#KEY).
        #[arg(long = "name", value_name = "KEY")]
        key: Option<String>,
    },
    /// The places a key is looked for: list, add or remove them.
    Source {
        #[command(subcommand)]
        cmd: KeySourceCmd,
    },
}

#[derive(Debug, Subcommand)]
pub enum KeySourceCmd {
    /// List where a key is looked for, in order (the saved key is last unless api_key places it) — every key's, when none is named.
    #[command(visible_alias = "ls")]
    List {
        name: String,
        /// The key (or write PROVIDER#KEY).
        #[arg(long = "name", value_name = "KEY")]
        key: Option<String>,
    },
    /// Add a place to look for a key (PROVIDER#KEY): at the end of its api_key, or first with --first (one already listed moves there). Lexi never writes to it.
    Add {
        name: String,
        #[command(flatten)]
        source: KeySourceArg,
        /// Look here before the other sources.
        #[arg(long)]
        first: bool,
        /// The key (or write PROVIDER#KEY).
        #[arg(long = "name", value_name = "KEY")]
        key: Option<String>,
    },
    /// Stop looking in a place for a key (PROVIDER#KEY; --saved takes out only its place in the list: the saved key is looked in last again).
    #[command(visible_aliases = ["del", "remove"])]
    Rm {
        name: String,
        #[command(flatten)]
        source: KeySourceArg,
        /// The key (or write PROVIDER#KEY).
        #[arg(long = "name", value_name = "KEY")]
        key: Option<String>,
    },
}

/// One place a key can be: exactly one of these.
#[derive(Debug, clap::Args)]
#[group(required = true, multiple = false)]
pub struct KeySourceArg {
    /// An environment variable.
    #[arg(long, value_name = "VAR")]
    pub env: Option<String>,
    /// A file holding the key (relative to the Lexi folder; ~/ for your home folder).
    #[arg(long, value_name = "PATH")]
    pub file: Option<String>,
    /// A program that prints the key, with its arguments, e.g. 'op read op://Private/OpenAI/credential' (run without a shell).
    #[arg(long, value_name = "COMMAND")]
    pub command: Option<String>,
    /// The saved key's place in the list.
    #[arg(long)]
    pub saved: bool,
}

/// Model metadata flags shared by `models add` / `models set`.
#[derive(Debug, clap::Args)]
pub struct ModelMeta {
    /// Display name.
    #[arg(long)]
    pub name: Option<String>,
    #[arg(long)]
    pub description: Option<String>,
    #[arg(long = "max_tokens", visible_alias = "max-tokens")]
    pub max_tokens: Option<u64>,
    #[arg(long = "context_window", visible_alias = "context-window")]
    pub context_window: Option<u64>,
    /// Make this the provider's default model.
    #[arg(long)]
    pub default: bool,
}

#[derive(Debug, Subcommand)]
pub enum ModelsCmd {
    /// List a provider's configured models.
    #[command(visible_alias = "ls")]
    List {
        provider: String,
        /// Print the raw configured-models.yaml.
        #[arg(short, long)]
        raw: bool,
        /// With --raw: also print the raw model-aliases.yaml.
        #[arg(short, long)]
        aliases: bool,
        /// Render as a table.
        #[arg(short, long)]
        table: bool,
        /// Include every metadata field.
        #[arg(short, long)]
        details: bool,
    },
    /// List the models a provider hosts (live call to its models endpoint).
    Hosted {
        provider: String,
        /// Print the models as JSON.
        #[arg(short, long)]
        raw: bool,
        /// Render as a table.
        #[arg(short, long)]
        table: bool,
        /// Only models with this tag (repeatable: all must match), e.g. out:image.
        #[arg(long = "tag", value_name = "TAG")]
        tags: Vec<String>,
        /// Use the last fetched listing instead of calling the provider.
        #[arg(long)]
        cached: bool,
        /// Show how the listing is fetched instead of fetching it: the models
        /// endpoint and the catalog settings (paging, fields, tag and limit
        /// rules), and whether they come from the provider's template or
        /// providers-config.yaml.
        #[arg(long, conflicts_with_all = ["table", "tags", "cached"])]
        schema: bool,
        /// List what this named key's account sees (or write PROVIDER#KEY).
        #[arg(long, value_name = "KEY")]
        key: Option<String>,
    },
    /// Show a provider's default model for each kind, or set one.
    ///
    /// A model becomes the default for its own kind — chat, image-generation,
    /// embedding, … (`models tags` shows a model's kind). Requests that don't
    /// name a model use the default for the kind of output they ask for.
    Default {
        provider: String,
        /// The model to make the default for its kind.
        model: Option<String>,
        /// Only this kind (chat, image-generation, embedding, …); with MODEL,
        /// the model must be of this kind.
        #[arg(long)]
        kind: Option<String>,
        /// Clear your default for --kind, so the provider's default applies.
        #[arg(long, requires = "kind", conflicts_with = "model")]
        clear: bool,
    },
    /// Manage a provider's model aliases.
    Alias {
        #[command(subcommand)]
        cmd: ModelAliasCmd,
    },
    /// Add a model (or update it if it exists).
    Add {
        provider: String,
        model: String,
        #[command(flatten)]
        meta: ModelMeta,
    },
    /// Update a model's metadata (or add it).
    Set {
        provider: String,
        model: String,
        #[command(flatten)]
        meta: ModelMeta,
    },
    /// Remove a model.
    #[command(visible_aliases = ["del", "delete"])]
    Rm { provider: String, model: String },
    /// Add or remove tags on a model id or glob: `+out:image`, `-in:image`.
    ///
    /// Tags describe what a model takes, produces and supports; they decide
    /// which of the provider's APIs serves it. Your tags win over the
    /// provider's tag rules. See `lexi tags` for the vocabulary.
    Tag {
        provider: String,
        /// A model id, or a glob such as "meta/*".
        model: String,
        /// Tag operations: `+tag` (or `tag`) adds, `-tag` removes.
        #[arg(allow_hyphen_values = true, required_unless_present = "clear")]
        ops: Vec<String>,
        /// Remove all of your tags on this model or glob.
        #[arg(long, conflicts_with = "ops")]
        clear: bool,
    },
    /// Show a model's effective tags, where each came from, and its API.
    Tags { provider: String, model: String },
}

#[derive(Debug, Subcommand)]
pub enum ModelAliasCmd {
    /// List a provider's model aliases.
    #[command(visible_alias = "ls")]
    List {
        provider: String,
        /// Print the raw model-aliases.yaml.
        #[arg(short, long)]
        raw: bool,
    },
    /// Point an alias at a model.
    Set {
        provider: String,
        alias: String,
        model: String,
    },
    /// Remove an alias.
    #[command(visible_aliases = ["del", "delete"])]
    Rm { provider: String, alias: String },
}

#[derive(Debug, Subcommand)]
pub enum ApiCmd {
    /// List API definitions (shipped and custom).
    #[command(visible_alias = "ls")]
    List {
        /// Print every definition as JSON.
        #[arg(short, long)]
        raw: bool,
    },
    /// Show an API definition.
    Show {
        name: String,
        /// Print the definition as JSON.
        #[arg(short, long)]
        raw: bool,
    },
    /// Validate an API definition (exit 1 if invalid).
    Validate { name: String },
    /// Create a custom API definition (scaffold, or a copy of --from).
    Create {
        name: String,
        /// Copy this definition (shipped or custom).
        #[arg(long)]
        from: Option<String>,
        /// Request path.
        #[arg(long)]
        path: Option<String>,
        /// Response text path (switches to simple text extraction).
        #[arg(long = "text-path")]
        text_path: Option<String>,
    },
    /// Edit a custom API definition.
    Edit {
        name: String,
        #[arg(long)]
        path: Option<String>,
        #[arg(long = "text-path")]
        text_path: Option<String>,
        /// Add response metadata: KEY:PATH. Repeatable.
        #[arg(long = "set-meta", value_name = "KEY:PATH")]
        set_meta: Vec<String>,
    },
    /// Remove a custom API definition.
    #[command(visible_aliases = ["del", "delete"])]
    Rm { name: String },
    /// Preview a definition: validation, CLI flags, tool presets, and a sample
    /// composed request body (always JSON).
    Preview {
        /// A definition by name (or use --file).
        #[arg(required_unless_present = "file", conflicts_with = "file")]
        name: Option<String>,
        /// Preview a YAML file instead of a saved definition.
        #[arg(long)]
        file: Option<PathBuf>,
        #[arg(long, default_value = "Sample prompt")]
        prompt: String,
        #[arg(long, default_value = "sample-model")]
        model: String,
        #[arg(long, default_value = "user")]
        role: String,
    },
    /// Save a YAML file as a custom definition (prints validation errors).
    Save { name: String, file: PathBuf },
}
