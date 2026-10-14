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

/** What the bridge sends a phone; after [Ready] the stream carries [ClientMsg] and [DaemonMsg]. */
@Serializable
@JsonClassDiscriminator("t")
sealed interface BridgeMsg {
    @Serializable @SerialName("paired")
    data class Paired(val device: String, val secret: String) : BridgeMsg

    @Serializable @SerialName("denied")
    data class DeniedMsg(val reason: Denied) : BridgeMsg

    @Serializable @SerialName("ready")
    data object Ready : BridgeMsg

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

    @Serializable @SerialName("create_worktree")
    data class CreateWorktree(val repo: String, val branch: String, val base: String? = null) : Cmd

    @Serializable @SerialName("remove_workspace")
    data class RemoveWorkspace(
        val workspace: Id,
        @SerialName("remove_worktree") val removeWorktree: Boolean = false,
    ) : Cmd

    @Serializable @SerialName("new_pane")
    data class NewPane(val workspace: Id, val program: List<String>? = null) : Cmd

    @Serializable @SerialName("close_pane")
    data class ClosePane(val pane: Id) : Cmd

    @Serializable @SerialName("write")
    data class Write(val pane: Id, val data: String) : Cmd

    @Serializable @SerialName("resize")
    data class Resize(val pane: Id, val rows: Int, val cols: Int) : Cmd

    /** A person looked at [node] at [rev]: the daemon ends the attentions raised up to it, for every client. */
    @Serializable @SerialName("mark_seen")
    data class MarkSeen(val node: Id, val rev: Long) : Cmd

    /** Turn the phone link on and make a pairing token; the result carries the link to pair from. */
    @Serializable @SerialName("mobile_start")
    data object MobileStart : Cmd

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

@Serializable
data class WorkspaceSummary(
    val id: Id,
    val name: String,
    val kind: WorkspaceKind,
    val path: String,
    val branch: String? = null,
    @SerialName("pane_count") val paneCount: Int,
    val attention: List<Attention>,
)

@Serializable
data class PaneSummary(val id: Id, val title: String, val attention: List<Attention>)

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
}

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
