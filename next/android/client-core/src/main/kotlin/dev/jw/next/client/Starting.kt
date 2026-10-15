package dev.jw.next.client

import dev.jw.next.contract.Block
import dev.jw.next.contract.BlocksMode
import dev.jw.next.contract.Node

/** Whether a command has drawn anything to see: a row of its output with more than blanks in it. */
fun Block.hasInk(): Boolean = out.any { row -> row.any { it.t.isNotBlank() } }

/** The program a command line names: the file name of its first word. */
fun programName(cmd: String): String = cmd.trim().split(Regex("\\s+")).first().substringAfterLast('/')

/**
 * The command that has the pane's screen and has painted nothing on it yet (REQ-259): the node is in
 * mode `full`, the command is running, it does not use the alternate screen (claude draws on the normal
 * one, so the pane's own screen would still be the shell's as it was) and its output is blank.
 */
fun Node.Blocks.starting(): Block? =
    blocks.lastOrNull()?.takeIf { mode == BlocksMode.Full && it.running && !it.full && !it.hasInk() }
