@file:OptIn(ExperimentalSerializationApi::class)

package dev.jw.next.contract

import kotlinx.serialization.ExperimentalSerializationApi
import kotlinx.serialization.SerialName
import kotlinx.serialization.Serializable
import kotlinx.serialization.json.Json
import kotlinx.serialization.json.JsonClassDiscriminator
import kotlinx.serialization.json.JsonElement
import kotlinx.serialization.json.JsonObject
import kotlinx.serialization.json.JsonPrimitive

/**
 * The JSON documents shared by the daemon and every client. The schema in `contract/schema`
 * and the fixtures in `contract/fixtures` are the contract; these types are the Kotlin side
 * and `ConformanceTest` keeps them honest. Unknown fields are ignored and absent optional
 * fields are not written, so a round trip reproduces a document exactly.
 */
val ContractJson = Json {
    ignoreUnknownKeys = true
    explicitNulls = false
    encodeDefaults = false
}

typealias Id = String

@Serializable
data class Peer(val name: String, val version: String)

@Serializable
enum class ErrorCode {
    @SerialName("bad_request") BadRequest,
    @SerialName("not_found") NotFound,
    @SerialName("unsupported") Unsupported,
    @SerialName("conflict") Conflict,
    @SerialName("git_failed") GitFailed,
    @SerialName("io") Io,
}

@Serializable
data class ErrorBody(val code: ErrorCode, val message: String)

@Serializable
enum class AttentionKind {
    @SerialName("question") Question,
    @SerialName("plan") Plan,
    @SerialName("permission") Permission,
    @SerialName("shell_done") ShellDone,
    @SerialName("agent_done") AgentDone,
    @SerialName("screen") Screen,
    @SerialName("merged") Merged,
    @SerialName("ci_failed") CiFailed,
}

@Serializable
enum class WorkspaceKind {
    @SerialName("worktree") Worktree,
    @SerialName("folder") Folder,
}

/** Something that wants a person; it names the node that opens when tapped. */
@Serializable
data class Attention(
    val kind: AttentionKind,
    val node: Id,
    val rev: Long,
    @SerialName("exit_code") val exitCode: Int? = null,
)

/** What a client sends. */
@Serializable
@JsonClassDiscriminator("t")
sealed interface ClientMsg {
    @Serializable
    @SerialName("hello")
    data class Hello(val client: Peer, val accepts: Map<String, List<Int>>) : ClientMsg

    @Serializable
    @SerialName("open")
    data class Open(val node: Id, val epoch: String? = null, val since: Long? = null) : ClientMsg

    @Serializable
    @SerialName("close")
    data class Close(val node: Id) : ClientMsg

    @Serializable
    @SerialName("cmd")
    data class Command(val id: String, val cmd: Cmd) : ClientMsg
}

/** What a phone sends the bridge before the daemon's protocol starts. */
@Serializable
@JsonClassDiscriminator("t")
sealed interface PhoneMsg {
    @Serializable @SerialName("pair")
    data class Pair(val token: String, val name: String, val code: String) : PhoneMsg

    @Serializable @SerialName("auth")
    data class Auth(val device: String, val secret: String) : PhoneMsg
}

/** Why the bridge refused a phone. */
@Serializable
enum class Denied {
    @SerialName("bad_token") BadToken,
    @SerialName("declined") Declined,
    @SerialName("timeout") Timeout,
    @SerialName("bad_secret") BadSecret,
    @SerialName("revoked") Revoked,
    @SerialName("locked_out") LockedOut,
}

/** What the bridge sends a phone; [Ready] says the PC's name, and after it the stream carries [ClientMsg] and [DaemonMsg]. */
@Serializable
@JsonClassDiscriminator("t")
sealed interface BridgeMsg {
    @Serializable @SerialName("paired")
    data class Paired(val device: String, val secret: String) : BridgeMsg

    @Serializable @SerialName("denied")
    data class DeniedMsg(val reason: Denied) : BridgeMsg

    @Serializable @SerialName("ready")
    data class Ready(val host: String? = null) : BridgeMsg

    /** Where the PC can be reached; sent after [Ready] and whenever it changes. */
    @Serializable @SerialName("endpoints")
    data class Endpoints(val lan: String? = null, val tunnel: String? = null) : BridgeMsg
}

/** A client action; the daemon answers with a `reply` carrying the same id. */
@Serializable
@JsonClassDiscriminator("cmd")
sealed interface Cmd {
    @Serializable @SerialName("rename_session")
    data class RenameSession(val name: String) : Cmd

    @Serializable @SerialName("open_folder")
    data class OpenFolder(val path: String) : Cmd

    /** Read the subfolders of [path] (the PC's home when none); the result is a [DirListing]. */
    @Serializable @SerialName("list_dir")
    data class ListDir(val path: String? = null, val hidden: Boolean = false) : Cmd

    @Serializable @SerialName("create_worktree")
    data class CreateWorktree(val repo: String, val branch: String, val base: String? = null, val folder: String? = null) : Cmd

    @Serializable @SerialName("remove_workspace")
    data class RemoveWorkspace(
        val workspace: Id,
        @SerialName("remove_worktree") val removeWorktree: Boolean = false,
    ) : Cmd

    @Serializable @SerialName("new_pane")
    data class NewPane(val workspace: Id, val program: List<String>? = null) : Cmd

    @Serializable @SerialName("rename_workspace")
    data class RenameWorkspace(val workspace: Id, val name: String) : Cmd

    @Serializable @SerialName("rename_pane")
    data class RenamePane(val pane: Id, val name: String) : Cmd

    @Serializable @SerialName("close_pane")
    data class ClosePane(val pane: Id) : Cmd

    @Serializable @SerialName("write")
    data class Write(val pane: Id, val data: String) : Cmd

    @Serializable @SerialName("resize")
    data class Resize(val pane: Id, val rows: Int, val cols: Int) : Cmd

    /** A person looked at [node] at [rev]: the daemon ends the attentions raised up to it, for every client. */
    @Serializable @SerialName("mark_seen")
    data class MarkSeen(val node: Id, val rev: Long) : Cmd

    /** Make (or find) the `diff` node of [workspace] for [scope]; [commit] names the commit of the `commit` scope. The result is `{node}`. */
    @Serializable @SerialName("open_diff")
    data class OpenDiff(val workspace: Id, val scope: DiffScope, val commit: String? = null) : Cmd

    /** Make (or find) the `commits` node of [workspace]. The result is `{node}`. */
    @Serializable @SerialName("open_commits")
    data class OpenCommits(val workspace: Id) : Cmd

    /** Read a `diff` node again now. */
    @Serializable @SerialName("refresh_diff")
    data class RefreshDiff(val node: Id) : Cmd

    /** Record that [path] of a `diff` node was looked at (or forget it), for every client. */
    @Serializable @SerialName("set_viewed")
    data class SetViewed(val node: Id, val path: String, val viewed: Boolean) : Cmd

    /** An agent in [pane] reported what it is doing through a hook; the daemon turns it into attention. */
    @Serializable @SerialName("agent_event")
    data class AgentEvent(
        val pane: Id,
        val event: dev.jw.next.contract.AgentEvent,
        @SerialName("notification_type") val notificationType: String? = null,
        val tool: String? = null,
    ) : Cmd

    /** Turn the phone link on and make a pairing token; the result carries the link to pair from. */
    @Serializable @SerialName("mobile_start")
    data object MobileStart : Cmd

    /** The link as it is now, with the pairing token of `mobile_start`. */
    @Serializable @SerialName("mobile_link")
    data object MobileLink : Cmd

    /** Wait up to [seconds] for a phone to ask to pair. */
    @Serializable @SerialName("mobile_wait")
    data class MobileWait(val seconds: Int) : Cmd

    @Serializable @SerialName("mobile_answer")
    data class MobileAnswer(val yes: Boolean) : Cmd

    @Serializable @SerialName("mobile_stop")
    data object MobileStop : Cmd
}

/** What the daemon sends. */
@Serializable
@JsonClassDiscriminator("t")
sealed interface DaemonMsg {
    @Serializable
    @SerialName("hello")
    data class Hello(
        val daemon: Peer,
        val epoch: String,
        val root: Id,
        /** Node type -> the version served; a type missing here is not served. */
        val accepted: Map<String, Int>,
    ) : DaemonMsg

    @Serializable
    @SerialName("snapshot")
    data class Snapshot(val node: Node) : DaemonMsg

    @Serializable
    @SerialName("patch")
    data class Patch(
        val node: Id,
        val epoch: String,
        val from: Long,
        val to: Long,
        val ops: List<PatchOp>,
    ) : DaemonMsg

    /** The node was removed, or never existed. */
    @Serializable
    @SerialName("gone")
    data class Gone(val node: Id) : DaemonMsg

    /** `ok` true carries an optional `result`; false carries `error`. */
    @Serializable
    @SerialName("reply")
    data class Reply(
        val id: String,
        val ok: Boolean,
        val result: JsonObject? = null,
        val error: ErrorBody? = null,
    ) : DaemonMsg

    /** A message the daemon could not act on that has no id to reply to. */
    @Serializable
    @SerialName("error")
    data class Error(val code: ErrorCode, val message: String) : DaemonMsg
}

/** One edit of a node document: the RFC 6902 subset the contract uses. */
@Serializable
@JsonClassDiscriminator("op")
sealed interface PatchOp {
    val path: String

    @Serializable @SerialName("add")
    data class Add(override val path: String, val value: JsonElement) : PatchOp

    @Serializable @SerialName("remove")
    data class Remove(override val path: String) : PatchOp

    @Serializable @SerialName("replace")
    data class Replace(override val path: String, val value: JsonElement) : PatchOp
}

/** One folder of a [DirListing]; [git] when it is a repository. */
@Serializable
data class DirEntry(val name: String, val git: Boolean = false)

/** The result of `list_dir`: [dirs] is cut to the first ones when [total] is more than they are. */
@Serializable
data class DirListing(val path: String, val parent: String? = null, val home: String, val dirs: List<DirEntry>, val total: Int)

@Serializable
data class WorkspaceSummary(
    val id: Id,
    val name: String,
    val kind: WorkspaceKind,
    val path: String,
    val project: String? = null,
    val branch: String? = null,
    @SerialName("pane_count") val paneCount: Int,
    val attention: List<Attention>,
)

@Serializable
data class PaneSummary(val id: Id, val title: String, val attention: List<Attention>)

/** The line the shell is holding: its text, and the cursor's place in it in characters. */
@Serializable
data class PaneInput(val text: String, val cursor: Int)

@Serializable
data class Program(val argv: List<String>, @SerialName("exit_code") val exitCode: Int? = null)

@Serializable
data class Cursor(val row: Int, val col: Int, val visible: Boolean)

@Serializable
data class Screen(val rows: Int, val cols: Int, val lines: List<String>, val cursor: Cursor)

/**
 * A node: one document of the tree, opened on demand and cached by clients. `v` is the
 * version of this node type's document; `epoch` changes whenever the daemon starts.
 */
@Serializable
@JsonClassDiscriminator("type")
sealed interface Node {
    val id: Id
    val v: Int
    val epoch: String
    val rev: Long

    /** Everything in this node's subtree that wants a person, computed by the daemon. */
    val attention: List<Attention>

    /** Level 0: the workspaces of the session, each with a summary. */
    @Serializable @SerialName("session")
    data class Session(
        override val id: Id,
        override val v: Int,
        override val epoch: String,
        override val rev: Long,
        override val attention: List<Attention>,
        val name: String,
        val workspaces: List<WorkspaceSummary>,
    ) : Node

    /** Level 1: one workspace and its panes. */
    @Serializable @SerialName("workspace")
    data class Workspace(
        override val id: Id,
        override val v: Int,
        override val epoch: String,
        override val rev: Long,
        override val attention: List<Attention>,
        val name: String,
        val kind: WorkspaceKind,
        val path: String,
        val project: String? = null,
        val branch: String? = null,
        val panes: List<PaneSummary>,
    ) : Node

    /** Level 2: one pane, a terminal. */
    @Serializable @SerialName("pane")
    data class Pane(
        override val id: Id,
        override val v: Int,
        override val epoch: String,
        override val rev: Long,
        override val attention: List<Attention>,
        val workspace: Id,
        val title: String,
        val shell: String,
        val cwd: String,
        val running: Program? = null,
        @SerialName("last_program") val lastProgram: Program? = null,
        val screen: Screen,
        /** The node of this pane's shell blocks, for a shell started with the integration. */
        val blocks: Id? = null,
        /** What the person has typed at the shell's prompt, when the shell says where it starts. */
        val input: PaneInput? = null,
    ) : Node

    /** Level 2 as command blocks: what a pane's shell ran, newest last. */
    @Serializable @SerialName("blocks")
    data class Blocks(
        override val id: Id,
        override val v: Int,
        override val epoch: String,
        override val rev: Long,
        override val attention: List<Attention>,
        val pane: Id,
        val mode: BlocksMode,
        val blocks: List<Block>,
    ) : Node

    /** A workspace's changes: its files, hunk by hunk, against a scope. */
    @Serializable @SerialName("diff")
    data class Diff(
        override val id: Id,
        override val v: Int,
        override val epoch: String,
        override val rev: Long,
        override val attention: List<Attention>,
        val workspace: Id,
        val scope: DiffScope,
        /** The commit of the `commit` scope. */
        val commit: String? = null,
        /** What `general` compares with, such as `origin/main`. */
        val base: String? = null,
        val state: DiffState,
        /** Over 2 MiB: the files are listed with no hunks. */
        @SerialName("too_large") val tooLarge: Boolean = false,
        /** Files left out of a too large diff. */
        val omitted: Int? = null,
        val files: List<DiffFile>,
    ) : Node

    /** The commits of a workspace's branch, newest first. */
    @Serializable @SerialName("commits")
    data class Commits(
        override val id: Id,
        override val v: Int,
        override val epoch: String,
        override val rev: Long,
        override val attention: List<Attention>,
        val workspace: Id,
        val base: String? = null,
        /** Older commits exist than the ones listed. */
        val more: Boolean = false,
        val commits: List<CommitInfo>,
    ) : Node
}

/** The hooks of an agent the daemon listens to. */
@Serializable
enum class AgentEvent {
    @SerialName("notification") Notification,
    @SerialName("pre_tool_use") PreToolUse,
    @SerialName("post_tool_use") PostToolUse,
    @SerialName("user_prompt_submit") UserPromptSubmit,
    @SerialName("stop") Stop,
    @SerialName("session_end") SessionEnd,
}

@Serializable
enum class DiffScope {
    @SerialName("uncommitted") Uncommitted,
    @SerialName("general") General,
    @SerialName("commit") Commit,
}

@Serializable
enum class DiffState {
    @SerialName("ok") Ok,
    /** `origin` has no default branch: a `general` diff has nothing to compare with. */
    @SerialName("no_base") NoBase,
}

@Serializable
enum class FileStatus {
    @SerialName("added") Added,
    @SerialName("modified") Modified,
    @SerialName("deleted") Deleted,
    @SerialName("renamed") Renamed,
    @SerialName("untracked") Untracked,
}

@Serializable
enum class Seen {
    /** Looked at, and unchanged since. */
    @SerialName("yes") Yes,
    /** Looked at, but the file differs now. */
    @SerialName("changed") Changed,
}

@Serializable
data class DiffFile(
    val path: String,
    @SerialName("old_path") val oldPath: String? = null,
    val status: FileStatus,
    val binary: Boolean = false,
    /** Why the daemon did not read the file (`symlink`, `too_big`). */
    val unreadable: String? = null,
    val added: Int,
    val deleted: Int,
    /** Fingerprint of the path, status and lines; a mark of having looked is only good while it stays. */
    val hash: String,
    val seen: Seen? = null,
    val hunks: List<Hunk>,
)

@Serializable
data class Hunk(
    /** What git prints after the second `@@`, usually the enclosing function. */
    val context: String,
    @SerialName("old_start") val oldStart: Int,
    @SerialName("old_lines") val oldLines: Int,
    @SerialName("new_start") val newStart: Int,
    @SerialName("new_lines") val newLines: Int,
    /** Each line prefixed with `+` (added), `-` (deleted) or a space (unchanged). */
    val lines: List<String>,
)

@Serializable
data class CommitInfo(
    val sha: String,
    val short: String,
    val subject: String,
    /** Commit time in Unix seconds. */
    val time: Long,
)

/** `Full` while a program has the whole screen: draw the pane's screen instead. */
@Serializable
enum class BlocksMode {
    @SerialName("blocks") Blocks,
    @SerialName("full") Full,
}

@Serializable
data class Block(
    /** Counts up per pane and is never reused. */
    val id: Long,
    val cmd: String,
    /** Where the command started. */
    val cwd: String,
    /** Unix milliseconds. */
    val started: Long,
    val running: Boolean,
    /** The exit status; absent while it runs. */
    val code: Int? = null,
    /** How long it took in milliseconds; absent while it runs. */
    val ms: Long? = null,
    /** The start of the output was dropped to stay within the limits. */
    val truncated: Boolean = false,
    /** The command used the whole screen; its output is not kept. */
    val full: Boolean = false,
    /** The output as rows of styled spans: the daemon resolves colours and attributes. */
    val out: List<List<Span>>,
)

/** A run of text with one style. [fg] and [bg] are a palette index (a number) or `#rrggbb` (a string). */
@Serializable
data class Span(
    val t: String,
    val fg: JsonPrimitive? = null,
    val bg: JsonPrimitive? = null,
    val b: Boolean = false,
    val i: Boolean = false,
    val u: Boolean = false,
)
