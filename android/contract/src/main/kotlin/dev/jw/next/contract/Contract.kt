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
    @SerialName("binary") Binary,
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
    /** An artifact of the workspace waits for a verdict (N171). */
    @SerialName("artifact") Artifact,
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
        val force: Boolean = false,
    ) : Cmd

    /** Count the commits of a worktree's branch that no remote has; the result is `{ahead}`. */
    @Serializable @SerialName("check_workspace")
    data class CheckWorkspace(val workspace: Id) : Cmd

    /** Push the worktree's branch to its remote; the result is `{branch, pushed, new_branch}`. */
    @Serializable @SerialName("sync_workspace")
    data class SyncWorkspace(val workspace: Id) : Cmd

    /** Forget a job that ended; one that runs is a `conflict`. */
    @Serializable @SerialName("dismiss_job")
    data class DismissJob(val job: Id) : Cmd

    /** Forget every job that ended. */
    @Serializable @SerialName("dismiss_jobs")
    data object DismissJobs : Cmd

    /** Stop what runs for the workspace and can be stopped: a push or a review being made. */
    @Serializable @SerialName("cancel")
    data class Cancel(val workspace: Id) : Cmd

    /** Read the settings of the repository [repo] is in (its `.jw.toml`); the result is a [ProjectInfo]. */
    @Serializable @SerialName("project_config")
    data class ProjectConfig(val repo: String) : Cmd

    /** The worktrees git knows of the repository [repo] is in, except its main one; the result is `{worktrees: [WorktreeInfo]}`. */
    @Serializable @SerialName("list_worktrees")
    data class ListWorktrees(val repo: String) : Cmd

    /** The open pull requests of the repository the workspace is in; with [requested], those that ask for the person's review. The result is a [PrList]. */
    @Serializable @SerialName("list_prs")
    data class ListPrs(val workspace: Id, val requested: Boolean = false) : Cmd

    /** Make, or bring up to date, the worktree that reviews pull request [pr]; the result is `{workspace, existed, dirty}`. */
    @Serializable @SerialName("review_pr")
    data class ReviewPr(val workspace: Id, val pr: Long) : Cmd

    /** Open a worktree [ListWorktrees] listed as a workspace with no panel; the result is the one of [CreateWorktree]. */
    @Serializable @SerialName("open_worktree")
    data class OpenWorktree(val repo: String, val path: String) : Cmd

    /** Start the one panel of a workspace that has none, as [open] says or, when it is null, as its settings do; the result is `{panes: [id]}`. */
    @Serializable @SerialName("start_workspace")
    data class StartWorkspace(val workspace: Id, val open: OpenKind? = null) : Cmd

    @Serializable @SerialName("new_pane")
    data class NewPane(val workspace: Id, val program: List<String>? = null, val beside: Id? = null, val dir: SplitDir? = null) : Cmd

    /** Swap the places of two panes of a workspace in its layout. */
    @Serializable @SerialName("swap_panes")
    data class SwapPanes(val a: Id, val b: Id) : Cmd

    /** Make a pane wider ([SplitDir.Row]) or taller ([SplitDir.Col]) by [by] thousandths; negative makes it narrower or lower. */
    @Serializable @SerialName("grow_pane")
    data class GrowPane(val pane: Id, val dir: SplitDir, val by: Int) : Cmd

    /** List one folder of a workspace ([path] is relative to its folder); the result is a file listing. */
    @Serializable @SerialName("list_files")
    data class ListFiles(val workspace: Id, val path: String? = null) : Cmd

    /** Find the files of a workspace whose path holds [query]; the result is the paths found. */
    /** The conversations Claude saved for the workspace's folder, newest first, up to 50. The result is a [ConversationList]. */
    @Serializable @SerialName("list_conversations")
    data class ListConversations(val workspace: Id) : Cmd

    /** A Markdown file of the workspace as blocks: [path] is relative to its folder. The result is a [MarkdownDoc]. */
    @Serializable @SerialName("read_markdown")
    data class ReadMarkdown(val workspace: Id, val path: String) : Cmd

    /** A text file of the workspace as lines: [path] is relative to its folder. The result is a [TextDoc]; a file that is not text is refused with [ErrorCode.Binary] (REQ-610 to REQ-614). */
    @Serializable @SerialName("read_file")
    data class ReadFile(val workspace: Id, val path: String) : Cmd

    /** The items of a conversation that came before the ones its node holds: up to [count] of those made before the time [before]. The result is an [OlderItems] (REQ-567). */
    @Serializable @SerialName("read_older")
    data class ReadOlder(val node: Id, val before: Long, val count: Int) : Cmd

    /** Write the settings of the repository [repo] is in to its own `.jw.toml` or to the personal file, after checking them. */
    @Serializable @SerialName("write_project_config")
    data class WriteProjectConfig(val repo: String, val to: ConfigFrom, val text: String) : Cmd

    /** Change some settings in the file [to] says, leaving the rest of the file as it was. */
    @Serializable @SerialName("edit_project_config")
    data class EditProjectConfig(val repo: String, val to: ConfigFrom, val set: ProjectEdit) : Cmd

    /** The plan that waits in a conversation, whole, as blocks. The result is a [MarkdownDoc]. */
    @Serializable @SerialName("read_plan")
    data class ReadPlan(val node: Id, val ask: String) : Cmd

    @Serializable @SerialName("find_files")
    data class FindFiles(val workspace: Id, val query: String) : Cmd

    @Serializable @SerialName("rename_workspace")
    data class RenameWorkspace(val workspace: Id, val name: String) : Cmd

    /** Make a group of workspaces; the reply is `{group}`. */
    @Serializable @SerialName("create_group")
    data class CreateGroup(val name: String) : Cmd

    @Serializable @SerialName("rename_group")
    data class RenameGroup(val group: Id, val name: String) : Cmd

    /** The group's workspaces stay open and belong to none. */
    @Serializable @SerialName("delete_group")
    data class DeleteGroup(val group: Id) : Cmd

    /** Put the workspace's repository in [group], or in none when it is null. */
    @Serializable @SerialName("move_to_group")
    data class MoveToGroup(val workspace: Id, val group: Id? = null) : Cmd

    /** [delta] is -1 (up) or 1 (down). */
    @Serializable @SerialName("move_group")
    data class MoveGroup(val group: Id, val delta: Int) : Cmd

    /** Swap the repository of [workspace] with its neighbour in its group (or among those in none): [delta] is -1 or 1. */
    @Serializable @SerialName("move_repo")
    data class MoveRepo(val workspace: Id, val delta: Int) : Cmd

    /** Make a conversation of its own: an empty folder, a workspace with `chat` and one panel. The reply is `{workspace}`. */
    @Serializable @SerialName("chat_new")
    data object ChatNew : Cmd

    /** Close a chat's panel and keep the chat; a chat with no message is removed (`{discarded: true}`). */
    @Serializable @SerialName("archive_chat")
    data class ArchiveChat(val workspace: Id) : Cmd

    /** Open a panel again in an archived chat's folder, on its saved session. The reply is `{pane}`. */
    @Serializable @SerialName("resume_chat")
    data class ResumeChat(val workspace: Id) : Cmd

    /** Remove a chat's folder and the chat; Claude's own transcripts are not touched. */
    @Serializable @SerialName("delete_chat")
    data class DeleteChat(val workspace: Id) : Cmd

    /** Start a new conversation in [to]; the reply is `{pane, text}`, the summary for the client's field unless [summary] is false. */
    @Serializable @SerialName("promote_chat")
    data class PromoteChat(val workspace: Id, val to: Id, val summary: Boolean? = null) : Cmd

    @Serializable @SerialName("rename_pane")
    data class RenamePane(val pane: Id, val name: String) : Cmd

    @Serializable @SerialName("close_pane")
    data class ClosePane(val pane: Id) : Cmd

    /** Open the diff or the viewer of a workspace as a leaf of its layout; replies `{view}`. */
    @Serializable @SerialName("open_view")
    data class OpenView(
        val workspace: Id,
        val kind: ViewKind,
        val path: String? = null,
        val beside: Id? = null,
        val dir: SplitDir? = null,
    ) : Cmd

    /** Close a view, or with [path] one tab of a viewer (the last tab closes it). */
    @Serializable @SerialName("close_view")
    data class CloseView(val view: Id, val path: String? = null) : Cmd

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

    /** Make (or find) the `tasks` node of [workspace] (N180). The result is `{node}`; no list is `not_found`. */
    @Serializable @SerialName("open_tasks")
    data class OpenTasks(val workspace: Id) : Cmd

    /** Change one task of the workspace's list: [text] is the reason of `block` and the note of `note`. */
    @Serializable @SerialName("tasks_edit")
    data class TasksEdit(val workspace: Id, val op: TaskOp, val id: String, val text: String? = null) : Cmd

    /** Turn on the loop over the workspace's list (N180): [max] laps (1 to 99, 20 when absent); [pane] is the agent's own. */
    @Serializable @SerialName("tasks_loop_on")
    data class TasksLoopOn(val workspace: Id, val max: Int? = null, val pane: Id? = null) : Cmd

    /** Turn the loop off. */
    @Serializable @SerialName("tasks_loop_off")
    data class TasksLoopOff(val workspace: Id) : Cmd

    /**
     * Publish an artifact (N171): the next version of the one named [name] in [workspace]. The result is
     * `{id, n, hash, new}`. Only the CLI sends it: the bridge refuses it from a phone.
     */
    @Serializable @SerialName("artifact_publish")
    data class ArtifactPublish(
        val workspace: Id,
        val name: String,
        val title: String? = null,
        val kind: ArtifactKind? = null,
        val format: ArtifactFormat,
        val text: String,
        /** The pane that publishes, where the verdict goes. */
        val pane: Id? = null,
    ) : Cmd

    /** Make (or find) the `artifacts` node of [workspace]. The result is `{node}`. */
    @Serializable @SerialName("open_artifacts")
    data class OpenArtifacts(val workspace: Id) : Cmd

    /** One version of an artifact, the latest by default. The result is an [ArtifactDoc]. */
    @Serializable @SerialName("artifact_read")
    data class ArtifactRead(val id: Id, val n: Int? = null) : Cmd

    /** The person's verdict on version [n], which must still be the latest and have that [hash]. */
    @Serializable @SerialName("artifact_decide")
    data class ArtifactDecide(
        val id: Id,
        val n: Int,
        val hash: String,
        @SerialName("type") val verdict: Verdict,
        /** What to change; a `changes` verdict needs it. */
        val text: String? = null,
    ) : Cmd

    /** Remove an artifact and all its versions. Only the CLI sends it. */
    @Serializable @SerialName("artifact_rm")
    data class ArtifactRm(val id: Id) : Cmd

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

    /** Stop the daemon the way it stops by itself; the PC's own `jw stop` sends it. */
    @Serializable @SerialName("stop")
    data object Stop : Cmd

    /**
     * Open the conversation of [pane] (it runs `claude`): a new one, or the one of [resume]; [args] are
     * the flags `claude` was typed with that the engine serves and [env] the environment it was typed in.
     * The result is `{node, session}`.
     */
    @Serializable @SerialName("open_conversation")
    data class OpenConversation(
        val pane: Id,
        val resume: String? = null,
        val args: List<String> = emptyList(),
        val env: Map<String, String>? = null,
    ) : Cmd

    /** Say something to Claude, or run a command of the `/` list. */
    @Serializable @SerialName("conversation_send")
    data class ConversationSend(
        val node: Id,
        val text: String,
        val images: List<Image> = emptyList(),
        /** The characters of [text], `[from, to]`, that were each one folded paste in the composer. */
        val folds: List<List<Int>> = emptyList(),
        /** The line began with a space or a tab: it is not kept in the history. */
        val private: Boolean = false,
        /** Interrupt the turn that runs and send this first. */
        val now: Boolean = false,
    ) : Cmd

    /** Take the last message of the person's that waits out of the queue. The result is an [Unqueued], or none. */
    @Serializable @SerialName("conversation_unqueue")
    data class ConversationUnqueue(val node: Id) : Cmd

    /** The messages sent from the repository of the conversation's folder, oldest first. The result is a [SentList]. */
    @Serializable @SerialName("conversation_history")
    data class ConversationHistory(val node: Id) : Cmd

    /** Answer the `ask` that waits: [key] is one of its options' keys; [text] goes with a refusal; [by] says where the answer comes from. */
    @Serializable @SerialName("conversation_answer")
    data class ConversationAnswer(val node: Id, val ask: String, val key: String, val text: String? = null, val by: String? = null) : Cmd

    /** Stop the turn that runs. */
    @Serializable @SerialName("conversation_interrupt")
    data class ConversationInterrupt(val node: Id) : Cmd

    /** Stop a task the engine runs in the background; [task] is the id in the node's tasks. */
    @Serializable @SerialName("conversation_stop_task")
    data class ConversationStopTask(val node: Id, val task: String) : Cmd

    /** Change what the conversation is called, its colour, the model, the effort or the mode. */
    @Serializable @SerialName("conversation_set")
    data class ConversationSet(
        val node: Id,
        val name: String? = null,
        val accent: Accent? = null,
        val model: String? = null,
        val effort: String? = null,
        val mode: ConvMode? = null,
    ) : Cmd

    /** Run a shell line of the person's (`!command`) in the pane's folder. */
    @Serializable @SerialName("conversation_shell")
    data class ConversationShell(val node: Id, val command: String) : Cmd

    /** End the conversation (`/exit`): the engine stops and the pane is the shell again. */
    @Serializable @SerialName("conversation_close")
    data class ConversationClose(val node: Id) : Cmd
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

/** What an entry of a workspace's folder is. */
@Serializable
enum class FileKind {
    @SerialName("dir") Dir,
    @SerialName("file") File,
    /** A symbolic link, whatever it points at: it is not followed. */
    @SerialName("link") Link,
}

/** What git says of an entry; of a folder, the most serious of what is under it. */
@Serializable
enum class GitMark {
    @SerialName("conflict") Conflict,
    @SerialName("modified") Modified,
    @SerialName("deleted") Deleted,
    @SerialName("added") Added,
    @SerialName("untracked") Untracked,
}

/** One entry of a [FileListing]; [size] in bytes for a file, [ignored] when git ignores it. */
@Serializable
data class FileEntry(
    val name: String,
    val kind: FileKind,
    val size: Long? = null,
    val git: GitMark? = null,
    val ignored: Boolean = false,
)

/** The result of `list_files`: [path] is relative to the workspace's folder; [entries] is cut to the first ones when [total] is more than they are. */
@Serializable
data class FileListing(val path: String, val entries: List<FileEntry>, val total: Int)

/** What the one panel of a workspace is: a shell or `claude` (REQ-504). */
@Serializable
enum class OpenKind {
    @SerialName("shell") Shell,
    @SerialName("claude") Claude,
}

/** Where a project's settings are written (REQ-482). */
@Serializable
enum class ConfigFrom {
    @SerialName("repo") Repo,
    @SerialName("personal") Personal,
}

/** The result of `project_config`: what a worktree will be made with and what a workspace opens with. */
@Serializable
data class ProjectInfo(
    val source: String? = null,
    val root: String,
    val setup: List<String>,
    /** What a workspace opens with: the one panel's program. */
    val open: OpenKind,
    /** Which file the settings are of: the repository's own or the personal one; none when the defaults are used. */
    val from: ConfigFrom? = null,
    /** Where the personal file is or would be. */
    val personal: String = "",
    /** The files of the checkout a new worktree gets a copy of. */
    val copy: List<String> = emptyList(),
    /** The names of the ports a worktree gets and their offsets in its block. */
    val ports: Map<String, Int> = emptyMap(),
    /** The `root` key as the file wrote it; none when the file has none. */
    @SerialName("root_text") val rootText: String? = null,
)

/** A worktree git knows of, as `list_worktrees` lists it. */
@Serializable
data class WorktreeInfo(
    val path: String,
    /** The branch it is on; none when its head is detached. */
    val branch: String? = null,
    /** The commit its head is at, the first seven digits. */
    val head: String,
    /** Its folder is not there any more. */
    val missing: Boolean = false,
    /** How many entries `git status --porcelain` lists; none when it could not be read or the folder is missing. */
    val changes: Int? = null,
    /** The workspace that has it open. */
    val workspace: Id? = null,
)

/** What makes a workspace the review of a pull request: its number and the branch it is to be merged into. */
@Serializable
data class Review(val pr: Long, val base: String)

/** A file a pull request changes. */
@Serializable
data class PrFile(val path: String, val additions: Int, val deletions: Int)

/** An open pull request, as `list_prs` lists it. */
@Serializable
data class PrInfo(
    val number: Long,
    val title: String,
    val author: String,
    val branch: String,
    val base: String,
    val draft: Boolean = false,
    val updated: String,
    /** GitHub's review decision: `APPROVED`, `CHANGES_REQUESTED` or `REVIEW_REQUIRED`. */
    val decision: String? = null,
    val additions: Int,
    val deletions: Int,
    val body: String = "",
    val url: String = "",
    val files: List<PrFile> = emptyList(),
)

/** The result of `list_prs`. */
@Serializable
data class PrList(val prs: List<PrInfo>)

/** The result of `list_worktrees`. */
@Serializable
data class WorktreeList(val worktrees: List<WorktreeInfo>)

/** What `edit_project_config` changes: each field that is there is the new value of its key; an empty one takes its key out. */
@Serializable
data class ProjectEdit(
    val root: String? = null,
    val setup: List<String>? = null,
    val copy: List<String>? = null,
    val ports: Map<String, Int>? = null,
    val open: OpenKind? = null,
)

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
    /** The workspace reviews a pull request. */
    val review: Review? = null,
    /** A conversation of the workspace works: it thinks, writes, runs a tool or compacts. */
    val working: Boolean = false,
    /** A worktree is being removed: nothing can be done with the workspace. */
    val removing: Boolean = false,
    /** The group of the workspace's repository, when it has one. */
    val group: Id? = null,
    /** The workspace is a conversation of its own. */
    val chat: ChatInfo? = null,
    /** The workspace has a task list (N180). */
    val tasks: TasksSummary? = null,
)

/** What makes a workspace a conversation of its own, and what Home draws of it without opening it. */
@Serializable
data class ChatInfo(
    /** The panel is closed and the chat waits to be resumed. */
    val archived: Boolean = false,
    /** When the last item was added to its conversation, in seconds: a chat is ordered by it. */
    val active: Long,
    /** The text of that item on one line, clipped to 80 characters. */
    val last: String? = null,
)

/** A name that gathers workspaces; it holds nothing open. */
@Serializable
data class Group(val id: Id, val name: String)

@Serializable
enum class JobKind {
    @SerialName("remove_workspace") RemoveWorkspace,
    @SerialName("sync_workspace") SyncWorkspace,
    @SerialName("update_claude") UpdateClaude,
}

@Serializable
enum class JobState {
    @SerialName("running") Running,
    @SerialName("done") Done,
    @SerialName("failed") Failed,
}

/** One slow thing the daemon does, in the session node. */
@Serializable
data class Job(
    val id: Id,
    val kind: JobKind,
    /** The workspace it is about; it is gone from the session when a removal ended well. Null for a job that is about none (`update_claude`). */
    val workspace: Id? = null,
    /** The workspace's name for a removal, the branch for a push, `claude <before> → <after>` for an update. */
    val name: String,
    val state: JobState,
    /** Milliseconds since the epoch of the daemon's machine. */
    @SerialName("started_at") val startedAt: Long,
    @SerialName("duration_ms") val durationMs: Long? = null,
    /** Why it failed, as the tool that failed said it. */
    val detail: String? = null,
    /** It can be stopped with `cancel`. */
    val cancellable: Boolean = false,
)

/** Which way a split puts its two sides: a row is side by side, a col one over the other. */
@Serializable
enum class SplitDir {
    @SerialName("row") Row,
    @SerialName("col") Col,
}

/** The panes of a workspace as a tree of splits; [Split.ratio] is the share of [Split.a] in thousandths. */
@Serializable
@JsonClassDiscriminator("t")
sealed interface Layout {
    @Serializable @SerialName("pane") data class Pane(val pane: Id) : Layout
    @Serializable @SerialName("split") data class Split(val dir: SplitDir, val ratio: Int, val a: Layout, val b: Layout) : Layout
}

@Serializable
data class PaneSummary(val id: Id, val title: String, val attention: List<Attention>)

@Serializable
enum class ViewKind {
    @SerialName("diff") Diff,
    @SerialName("viewer") Viewer,
}

/** A leaf of a workspace's layout that is not a terminal: the diff, or a viewer with its tabs. */
@Serializable
data class ViewSummary(
    val id: Id,
    val kind: ViewKind,
    val title: String,
    /** A viewer's tabs: paths relative to the workspace, at most 8, the oldest first. */
    val files: List<String> = emptyList(),
)

/** The line the shell is holding: its text, and the cursor's place in it in characters. */
@Serializable
data class PaneInput(val text: String, val cursor: Int)

@Serializable
data class Program(val argv: List<String>, @SerialName("exit_code") val exitCode: Int? = null)

@Serializable
data class Cursor(val row: Int, val col: Int, val visible: Boolean)

@Serializable
/** [alt]: the program is on the alternate screen (nvim, htop, claude…). */
data class Screen(
    val rows: Int,
    val cols: Int,
    val lines: List<String>,
    val cursor: Cursor,
    val alt: Boolean = false,
    /** While [alt]: the same rows with their colours, exactly [rows] of them; empty otherwise. */
    val spans: List<List<Span>> = emptyList(),
)

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
        /** The slow things the daemon does or did, newest first. */
        val jobs: List<Job> = emptyList(),
        /** The groups of workspaces, in the order they are shown. */
        val groups: List<Group> = emptyList(),
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
        val layout: Layout? = null,
        /** The diff and the viewer that are leaves of [layout] beside the panes. */
        val views: List<ViewSummary> = emptyList(),
        /** The workspace reviews a pull request. */
        val review: Review? = null,
        /** A worktree is being removed: nothing can be done with the workspace. */
        val removing: Boolean = false,
        /** The workspace is a conversation of its own. */
        val chat: ChatInfo? = null,
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
        /** The node of this pane's conversation while one is open: the pane runs `claude`. */
        val conversation: Id? = null,
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

    /** A pane that runs `claude`, as a conversation: what each client draws in place of the screen. */
    @Serializable @SerialName("conversation")
    data class Conversation(
        override val id: Id,
        override val v: Int,
        override val epoch: String,
        override val rev: Long,
        override val attention: List<Attention>,
        val pane: Id,
        /** The engine's id for this conversation; what `--resume` takes. */
        val session: String? = null,
        val name: String,
        val accent: Accent,
        val model: String,
        val effort: String,
        val mode: ConvMode,
        val status: ConvStatus,
        /** Unix milliseconds at which the turn that runs began. */
        val since: Long? = null,
        /** How many items came before `items[0]`: the node keeps the last 300. */
        val first: Int,
        val items: List<ConvItem>,
        val ask: Ask? = null,
        /** The messages that wait for the turn that runs to end. */
        val queue: List<String>,
        val tasks: List<Task>,
        val todos: List<Todo>,
        val artifacts: List<Artifact>,
        val commands: List<ConvCommand>,
        /** Millionths of a dollar. */
        val cost: Long,
        val limits: Limits? = null,
        /** The tools that run, and when each began (REQ-562). */
        val running: List<Running> = emptyList(),
        /** The time of the first item of [items] when the conversation has a transcript that may hold older ones: the `before` of `read_older` (REQ-567). */
        val older: Long? = null,
        /** Another `claude` that has this session open, while the conversation has no engine of its own (REQ-575). */
        val held: Held? = null,
        /** The folder the engine runs in, the one its `@` references are written from (REQ-630, REQ-631). */
        val cwd: String? = null,
        /** The version of `claude` its engine says it is (REQ-852). */
        @SerialName("engine_version") val engineVersion: String? = null,
        /** The version of `claude` that is installed, while it is not the engine's (REQ-852). */
        @SerialName("engine_newer") val engineNewer: String? = null,
        /** What Claude proposes the person writes next, while the engine is stopped and nothing waits (REQ-572). */
        val suggestion: String? = null,
        /** When the turn began that another `claude` runs in the transcript this conversation follows (REQ-565). */
        val following: Long? = null,
        /** What the turn that runs has written, in tokens; 0 when the engine has not said (REQ-563). */
        val tokens: Long = 0,
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

    /** The task list of a workspace (N180): its tasks, what is wrong in the file, and the loop. */
    @Serializable @SerialName("tasks")
    data class Tasks(
        override val id: Id,
        override val v: Int,
        override val epoch: String,
        override val rev: Long,
        override val attention: List<Attention>,
        val workspace: Id,
        val path: String,
        val tasks: List<TaskItem>,
        val problems: List<TaskProblem> = emptyList(),
        @SerialName("loop") val loop: LoopInfo,
    ) : Node

    /** The artifacts a workspace has (N171), each with its versions. */
    @Serializable @SerialName("artifacts")
    data class Artifacts(
        override val id: Id,
        override val v: Int,
        override val epoch: String,
        override val rev: Long,
        override val attention: List<Attention>,
        val workspace: Id,
        val items: List<ArtifactItem>,
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

/** What the list's file says of a task: `[ ]`, `[x]`, `[!]`. */
@Serializable
enum class TaskStatus {
    @SerialName("todo") Todo,
    @SerialName("done") Done,
    @SerialName("blocked") Blocked,
}

/** What a task is at the moment; `running` is the loop's and is never in the file. */
@Serializable
enum class TaskPhase {
    @SerialName("waiting") Waiting,
    @SerialName("ready") Ready,
    @SerialName("running") Running,
    @SerialName("done") Done,
    @SerialName("blocked") Blocked,
}

/** What `tasks_edit` changes. */
@Serializable
enum class TaskOp {
    @SerialName("done") Done,
    @SerialName("block") Block,
    @SerialName("reopen") Reopen,
    @SerialName("note") Note,
}

/** Why a loop is not running. */
@Serializable
enum class StopKind {
    @SerialName("done") Done,
    @SerialName("blocked") Blocked,
    @SerialName("idle") Idle,
    @SerialName("max") Max,
    @SerialName("stuck") Stuck,
    @SerialName("person") Person,
    @SerialName("pane") Pane,
}

/** One task. [after] is the ids it waits for, or `["*"]` for every other task. */
@Serializable
data class TaskItem(
    val id: String,
    val title: String,
    val after: List<String>,
    val status: TaskStatus,
    val state: TaskPhase,
    val level: Int,
    val why: String? = null,
    val notes: List<String> = emptyList(),
    /** The last task, which waits for every other: the tests and their evidence. */
    @SerialName("final") val isFinal: Boolean = false,
    /** The final task is not in the file yet. */
    val auto: Boolean = false,
)

/** Something wrong in the file, which the daemon reports and does not mend. */
@Serializable
data class TaskProblem(val line: Int? = null, val text: String)

@Serializable
data class LoopStop(val kind: StopKind, val text: String)

/** The loop of a workspace's agent over the list (it lives in the daemon's memory). */
@Serializable
data class LoopInfo(
    val on: Boolean,
    val laps: Int,
    val max: Int,
    /** The id of the task handed out now. */
    val task: String? = null,
    val pane: Id? = null,
    /** The agent waits for the person. */
    val waiting: Boolean = false,
    val stop: LoopStop? = null,
)

/** What a workspace's row says of its list; [total] counts the final task even while it is not in the file. */
@Serializable
data class TasksSummary(
    val done: Int,
    val total: Int,
    val on: Boolean = false,
    val stop: LoopStop? = null,
)

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

/** What an artifact is for; only drawn on the list. */
@Serializable
enum class ArtifactKind {
    @SerialName("prototype") Prototype,
    @SerialName("document") Document,
    @SerialName("report") Report,
    @SerialName("spec") Spec,
}

/** `html` is drawn in an isolated WebView, `md` with the Markdown viewer. */
@Serializable
enum class ArtifactFormat {
    @SerialName("html") Html,
    @SerialName("md") Md,
}

@Serializable
enum class Verdict {
    @SerialName("approved") Approved,
    @SerialName("changes") Changes,
}

/** Whether the verdict reached the agent that published. */
@Serializable
enum class Delivered {
    @SerialName("agent") Agent,
    @SerialName("none") Nobody,
}

@Serializable
data class ArtifactVerdict(
    @SerialName("type") val verdict: Verdict,
    /** What to change; only a `changes` verdict has it. */
    val text: String? = null,
    /** Unix seconds. */
    val at: Long,
    /** Absent until the daemon has tried to tell the agent. */
    val delivered: Delivered? = null,
)

@Serializable
data class ArtifactVersion(
    /** From 1; a new version of the same name is the next number. */
    val n: Int,
    /** SHA-256 of the bytes, in lower-case hex. */
    val hash: String,
    val size: Long,
    /** Unix seconds. */
    val at: Long,
    /** The pane that published, where the verdict goes. */
    val publisher: Id? = null,
    val verdict: ArtifactVerdict? = null,
)

/** The result of `artifact_read` (N171): `html` for an `html` artifact, `doc` for a `md` one. */
@Serializable
data class ArtifactDoc(
    val id: Id,
    val n: Int,
    val hash: String,
    val title: String,
    val kind: ArtifactKind,
    val format: ArtifactFormat,
    val html: String? = null,
    val doc: MarkdownDoc? = null,
)

/** One artifact of a workspace, its versions oldest first. Its status is its latest version's verdict. */
@Serializable
data class ArtifactItem(
    val id: Id,
    val name: String,
    val title: String,
    val kind: ArtifactKind,
    val format: ArtifactFormat,
    val versions: List<ArtifactVersion>,
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
    /** The daemon restarted while it ran: its end was never seen; [ms] is how long it had run when the daemon last saved. */
    val interrupted: Boolean = false,
    /** Not a command: where the daemon restarted ([started] is when); what is before it was kept from before. */
    val restart: Boolean = false,
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
