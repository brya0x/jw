package dev.jw.next.contract

import kotlinx.serialization.Serializable

/*
 * A text file as lines of runs (spec: *The file viewer*): what `read_file` replies. A run is a
 * piece of a line with, when the daemon knows the language, its syntax class; the phone draws
 * the class with its theme's accent and parses nothing.
 */

/** A file: its [title], the canonical [path] it was read from, its [size] in bytes and its [lines]; [truncated] when the daemon read only the head. */
@Serializable
data class TextDoc(
    val title: String,
    val path: String,
    val size: Long,
    val truncated: Boolean = false,
    val lines: List<List<Run>>,
)

/** A piece of a line; [k] is `kw`, `str`, `num`, `ty`, `fn`, `cm` or `attr`, and a reader shows a missing or unknown one as plain text. */
@Serializable
data class Run(val t: String, val k: String? = null)
