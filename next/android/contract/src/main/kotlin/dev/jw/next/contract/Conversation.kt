@file:OptIn(ExperimentalSerializationApi::class)

package dev.jw.next.contract

import kotlinx.serialization.ExperimentalSerializationApi
import kotlinx.serialization.SerialName
import kotlinx.serialization.Serializable
import kotlinx.serialization.json.JsonClassDiscriminator

/*
 * The parts of a `conversation` node (spec: *The conversation*). Money is whole millionths of
 * a dollar, never a float, so a document round-trips byte for byte in every language.
 */

/** The conversation's colour: one of the theme's six accents. */
@Serializable
enum class Accent {
    @SerialName("blue") Blue,
    @SerialName("green") Green,
    @SerialName("yellow") Yellow,
    @SerialName("red") Red,
    @SerialName("magenta") Magenta,
    @SerialName("cyan") Cyan,
}

/** How much the engine may do without asking; [flag] is what `--permission-mode` takes. */
@Serializable
enum class ConvMode(val flag: String) {
    @SerialName("manual") Manual("manual"),
    @SerialName("acceptEdits") AcceptEdits("acceptEdits"),
    @SerialName("plan") Plan("plan"),
    @SerialName("auto") Auto("auto"),
}

@Serializable
enum class ConvStatus {
    @SerialName("idle") Idle,
    @SerialName("resuming") Resuming,
    @SerialName("thinking") Thinking,
    @SerialName("streaming") Streaming,
    @SerialName("tool") Tool,
    @SerialName("asking") Asking,
    @SerialName("compacting") Compacting,
    @SerialName("dead") Dead,
}

@Serializable
enum class ToolState {
    @SerialName("run") Run,
    @SerialName("ask") Ask,
    @SerialName("ok") Ok,
    @SerialName("denied") Denied,
    @SerialName("failed") Failed,
}

@Serializable
enum class NoteTone {
    @SerialName("warn") Warn,
    @SerialName("error") Error,
}

@Serializable
enum class DividerKind {
    @SerialName("restart") Restart,
    @SerialName("compact") Compact,
    @SerialName("clear") Clear,
    @SerialName("resume") Resume,
}

@Serializable
enum class DiffSign {
    @SerialName("+") Add,
    @SerialName("-") Del,
    @SerialName(" ") Same,
}

/** A line of a diff; [n] is its number in the file when it is known. */
@Serializable
data class DiffLine(val n: Int? = null, val sign: DiffSign, val text: String)

/** One thing in a conversation, oldest first. */
@Serializable
@JsonClassDiscriminator("t")
sealed interface ConvItem {
    /** What the person sent; [images] counts the pictures that went with it. */
    @Serializable @SerialName("user")
    data class User(val text: String, val images: Int = 0) : ConvItem

    /** What Claude wrote; [live] while it is still arriving. */
    @Serializable @SerialName("text")
    data class Text(val text: String, val live: Boolean = false) : ConvItem

    @Serializable @SerialName("tool")
    data class Tool(
        val id: String,
        val name: String,
        val arg: String,
        val state: ToolState,
        val summary: String? = null,
        val output: List<String> = emptyList(),
        val diff: List<DiffLine> = emptyList(),
        val url: String? = null,
        /** Where the answer to its permission came from. */
        val by: String? = null,
    ) : ConvItem

    /** A subagent: a card with its progress and its totals. */
    @Serializable @SerialName("agent")
    data class Agent(
        val id: String,
        val name: String,
        val description: String,
        val state: ToolState,
        val tools: Int,
        val tokens: Long,
        val ms: Long,
        val last: String? = null,
        val steps: List<String>,
    ) : ConvItem

    /** A shell line of the person's (`!command`) and what it printed. */
    @Serializable @SerialName("shell")
    data class Shell(val command: String, val state: ToolState, val output: List<String>) : ConvItem

    /** What a command answered, as lines. */
    @Serializable @SerialName("out")
    data class Out(val lines: List<String>) : ConvItem

    @Serializable @SerialName("note")
    data class Note(val text: String, val tone: NoteTone? = null) : ConvItem

    /** The end of a turn: how long it took, how many tools it used and what it cost. */
    @Serializable @SerialName("result")
    data class Result(val ms: Long, val tools: Int, val cost: Long) : ConvItem

    @Serializable @SerialName("divider")
    data class Divider(val text: String, val kind: DividerKind) : ConvItem
}

@Serializable
enum class AskKind {
    @SerialName("permission") Permission,
    @SerialName("question") Question,
    @SerialName("plan") Plan,
}

@Serializable
data class AskOption(val key: String, val label: String)

/** What waits for the person: a permission to use a tool, a choice Claude asks for, or a plan. */
@Serializable
data class Ask(
    /** What an answer names. */
    val id: String,
    val kind: AskKind,
    val tool: String,
    val title: String,
    val command: String? = null,
    val diff: List<DiffLine> = emptyList(),
    val plan: List<String> = emptyList(),
    val options: List<AskOption>,
    val multi: Boolean = false,
)

@Serializable
enum class TaskKind {
    @SerialName("agent") Agent,
    @SerialName("bash") Bash,
    @SerialName("monitor") Monitor,
    @SerialName("loop") Loop,
    @SerialName("other") Other,
}

@Serializable
enum class TaskState {
    @SerialName("run") Run,
    @SerialName("done") Done,
    @SerialName("stopped") Stopped,
    @SerialName("failed") Failed,
}

/** What the engine runs in the background. */
@Serializable
data class Task(val id: String, val kind: TaskKind, val label: String, val state: TaskState, val started: Long, val info: String? = null)

@Serializable
data class Todo(val text: String, val done: Boolean)

@Serializable
data class Artifact(val title: String, val url: String)

@Serializable
enum class CommandSource {
    @SerialName("jwn") Jwn,
    @SerialName("claude") Claude,
    @SerialName("project") Project,
    @SerialName("skill") Skill,
    @SerialName("plugin") Plugin,
}

@Serializable
enum class CommandKind {
    @SerialName("local") Local,
    @SerialName("panel") Panel,
    @SerialName("text") Text,
    @SerialName("prompt") Prompt,
    @SerialName("screen") Screen,
}

/** A command of the `/` list. */
@Serializable
data class ConvCommand(val name: String, val source: CommandSource, val kind: CommandKind, val description: String? = null)

/** How much of the subscription's windows is used, in whole percent. */
@Serializable
data class Limits(@SerialName("five_hour") val fiveHour: Int? = null, @SerialName("seven_day") val sevenDay: Int? = null)

/** What `open_conversation` answers: the conversation's node and the engine's session. */
@Serializable
data class Opened(val node: Id, val session: String)

/** A picture sent with a message: already small (a JPEG of at most 1568 pixels on its long side); [data] is base64. */
@Serializable
data class Image(@SerialName("media_type") val mediaType: String, val data: String)

/** One conversation Claude saved for a folder: what `/resume` lists. [at] is milliseconds since the epoch. */
@Serializable
data class SavedConversation(val id: String, val name: String, val at: Long)

/** What `list_conversations` answers, newest first. */
@Serializable
data class ConversationList(val conversations: List<SavedConversation>)
