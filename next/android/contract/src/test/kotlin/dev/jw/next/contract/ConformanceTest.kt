package dev.jw.next.contract

import com.fasterxml.jackson.databind.ObjectMapper
import com.fasterxml.jackson.databind.node.ObjectNode
import com.networknt.schema.JsonSchema
import com.networknt.schema.JsonSchemaFactory
import com.networknt.schema.SpecVersion
import java.io.File
import kotlinx.serialization.KSerializer
import kotlinx.serialization.builtins.ListSerializer
import kotlinx.serialization.json.Json
import kotlinx.serialization.json.JsonArray
import kotlinx.serialization.json.JsonElement
import kotlinx.serialization.json.JsonObject
import kotlinx.serialization.json.JsonPrimitive
import kotlinx.serialization.json.boolean
import kotlinx.serialization.json.contentOrNull
import kotlinx.serialization.json.jsonArray
import kotlinx.serialization.json.jsonObject
import kotlinx.serialization.json.jsonPrimitive
import kotlinx.serialization.json.long
import kotlin.test.Test
import kotlin.test.assertEquals
import kotlin.test.assertFailsWith
import kotlin.test.assertTrue
import kotlin.test.fail

/**
 * Keeps the Kotlin types and the shared fixtures from drifting apart. This is the same
 * suite as `crates/contract/tests/conformance.rs`, reading the same files (REQ-31, REQ-32 in
 * `next/docs/spec.md`).
 */
class ConformanceTest {
    private val contractDir = File(System.getProperty("contract.dir") ?: error("contract.dir is not set"))
    private val mapper = ObjectMapper()

    private fun fixtures(dir: String): List<Pair<String, JsonElement>> {
        val files = File(contractDir, "fixtures/$dir").listFiles { f -> f.extension == "json" }.orEmpty().sortedBy { it.name }
        assertTrue(files.isNotEmpty(), "no fixtures in $dir")
        return files.map { it.name to Json.parseToJsonElement(it.readText()) }
    }

    private val schemaText = File(contractDir, "schema/contract.schema.json").readText()
    private val schemaJson: JsonObject = Json.parseToJsonElement(schemaText).jsonObject

    private fun validator(def: String): JsonSchema {
        val root = mapper.readTree(schemaText) as ObjectNode
        root.put("\$ref", "#/\$defs/$def")
        return JsonSchemaFactory.getInstance(SpecVersion.VersionFlag.V202012).getSchema(root)
    }

    private fun errors(schema: JsonSchema, doc: JsonElement): List<String> =
        schema.validate(mapper.readTree(doc.toString())).map { it.message }

    private fun <T> roundTrip(name: String, serializer: KSerializer<T>, doc: JsonElement) {
        val typed = try {
            ContractJson.decodeFromJsonElement(serializer, doc)
        } catch (e: Exception) {
            fail("$name: Kotlin cannot parse it: ${e.message}")
        }
        assertEquals(doc, ContractJson.encodeToJsonElement(serializer, typed), "$name: Kotlin writes a different document")
    }

    @Test
    fun clientFixturesRoundTripAndValidate() {
        val schema = validator("ClientMsg")
        for ((name, doc) in fixtures("client")) {
            assertEquals(emptyList(), errors(schema, doc), "client/$name breaks the schema")
            roundTrip("client/$name", ClientMsg.serializer(), doc)
        }
    }

    @Test
    fun daemonFixturesRoundTripAndValidate() {
        val schema = validator("DaemonMsg")
        for ((name, doc) in fixtures("daemon")) {
            assertEquals(emptyList(), errors(schema, doc), "daemon/$name breaks the schema")
            roundTrip("daemon/$name", DaemonMsg.serializer(), doc)
        }
    }

    @Test
    fun phoneAndBridgeFixturesRoundTripAndValidate() {
        val phone = validator("PhoneMsg")
        for ((name, doc) in fixtures("phone")) {
            assertEquals(emptyList(), errors(phone, doc), "phone/$name breaks the schema")
            roundTrip("phone/$name", PhoneMsg.serializer(), doc)
        }
        val bridge = validator("BridgeMsg")
        for ((name, doc) in fixtures("bridge")) {
            assertEquals(emptyList(), errors(bridge, doc), "bridge/$name breaks the schema")
            roundTrip("bridge/$name", BridgeMsg.serializer(), doc)
        }
    }

    @Test
    fun invalidFixturesAreRejectedBySchema() {
        val validators = listOf("client", "phone", "bridge").associateWith { validator(it.replaceFirstChar(Char::uppercase) + "Msg") }
        val daemon = validator("DaemonMsg")
        for ((name, doc) in fixtures("invalid")) {
            val schema = validators[name.substringBefore('-')] ?: daemon
            assertTrue(errors(schema, doc).isNotEmpty(), "invalid/$name passed the schema")
        }
    }

    @Test
    fun unknownFieldsAreIgnored() {
        val doc = fixtures("daemon").first { it.first == "snapshot-pane.json" }.second.jsonObject
        val node = doc.getValue("node").jsonObject
        val extended = JsonObject(doc + ("future_top" to JsonPrimitive(true)) +
            ("node" to JsonObject(node + ("future_field" to JsonPrimitive(1)))))
        ContractJson.decodeFromJsonElement(DaemonMsg.serializer(), extended)
        assertEquals(emptyList(), errors(validator("DaemonMsg"), extended))
    }

    /** Collect every `const` of [prop] in a def's `oneOf` branches, or its `enum` values. */
    private fun schemaValues(def: String, prop: String): Set<String> {
        val d = schemaJson.getValue("\$defs").jsonObject.getValue(def).jsonObject
        val out = mutableSetOf<String>()
        d["oneOf"]?.jsonArray?.forEach { branch ->
            branch.jsonObject["properties"]?.jsonObject?.get(prop)?.jsonObject?.get("const")?.jsonPrimitive?.contentOrNull
                ?.let(out::add)
        }
        d["enum"]?.jsonArray?.forEach { out.add(it.jsonPrimitive.content) }
        return out
    }

    private fun collect(doc: JsonElement, key: String, out: MutableSet<String>) {
        when (doc) {
            is JsonObject -> doc.forEach { (k, v) ->
                if (k == key && v is JsonPrimitive && v.isString) out.add(v.content)
                collect(v, key, out)
            }
            is JsonArray -> doc.forEach { collect(it, key, out) }
            else -> {}
        }
    }

    private fun found(dir: String, key: String): Set<String> =
        buildSet { fixtures(dir).forEach { collect(it.second, key, this) } }

    /** A variant in the schema with no valid fixture is an untested corner. */
    @Test
    fun everySchemaVariantHasAFixture() {
        val cases = listOf(
            listOf("ClientMsg", "t", "client", "t"),
            listOf("DaemonMsg", "t", "daemon", "t"),
            listOf("Cmd", "cmd", "client", "cmd"),
            listOf("PatchOp", "op", "daemon", "op"),
            listOf("AttentionKind", "", "daemon", "kind"),
            listOf("NodeType", "", "daemon", "type"),
            listOf("ErrorCode", "", "daemon", "code"),
            listOf("PhoneMsg", "t", "phone", "t"),
            listOf("BridgeMsg", "t", "bridge", "t"),
            listOf("Denied", "", "bridge", "reason"),
        )
        for ((def, prop, dir, key) in cases) {
            val want = schemaValues(def, prop)
            assertTrue(want.isNotEmpty(), "$def: nothing found in the schema")
            val missing = want - found(dir, key)
            assertTrue(missing.isEmpty(), "$def: no fixture in $dir/ for $missing")
        }
    }

    @Test
    fun kotlinEnumsMatchTheSchema() {
        val kinds = schemaValues("AttentionKind", "")
        for (k in kinds) ContractJson.decodeFromString(AttentionKind.serializer(), "\"$k\"")
        for (c in schemaValues("ErrorCode", "")) ContractJson.decodeFromString(ErrorCode.serializer(), "\"$c\"")
        assertEquals(kinds.size, AttentionKind.entries.size, "Kotlin has an attention kind the schema lacks")
        assertEquals(schemaValues("ErrorCode", "").size, ErrorCode.entries.size, "Kotlin has an error code the schema lacks")
        assertEquals(schemaValues("WorkspaceKind", "").size, WorkspaceKind.entries.size)
        assertEquals(schemaValues("Denied", "").size, Denied.entries.size, "Kotlin has a denial the schema lacks")
    }

    /** Every patch case applies the same way as in Rust: `after`, or an error. */
    @Test
    fun patchCasesApplyAsWritten() {
        for ((name, case) in fixtures("patch-cases")) {
            val c = case.jsonObject
            val ops = ContractJson.decodeFromJsonElement(
                ListSerializer(PatchOp.serializer()),
                c.getValue("ops"),
            )
            val to = c.getValue("to").jsonPrimitive.long
            val before = c.getValue("before")
            if (c["error"]?.jsonPrimitive?.boolean == true) {
                assertFailsWith<PatchException>("patch-cases/$name should fail") { applyPatch(before, ops, to) }
            } else {
                assertEquals(c.getValue("after"), applyPatch(before, ops, to), "patch-cases/$name")
            }
        }
    }

    /** The blocks patch applies to the blocks snapshot and leaves a valid node. */
    @Test
    fun aBlocksPatchFinishesTheRunningBlock() {
        val snap = fixtures("daemon").first { it.first == "snapshot-blocks.json" }.second.jsonObject.getValue("node")
        val patch = fixtures("daemon").first { it.first == "patch-blocks-append.json" }.second
        val msg = ContractJson.decodeFromJsonElement(DaemonMsg.serializer(), patch) as DaemonMsg.Patch
        val node = ContractJson.decodeFromJsonElement(Node.serializer(), applyPatch(snap, msg.ops, msg.to)) as Node.Blocks
        val last = node.blocks.last()
        assertEquals(5L, node.rev)
        assertEquals(false, last.running)
        assertEquals(0, last.code)
        assertEquals(12400L, last.ms)
        assertEquals("test result: ok. 14 passed", last.out.last().first().t)
    }

    /** A patch from a daemon snapshot fixture stays a valid node of the same type. */
    @Test
    fun aPatchedSnapshotStillParsesAsANode() {
        val snap = fixtures("daemon").first { it.first == "snapshot-pane.json" }.second.jsonObject.getValue("node")
        val patch = fixtures("daemon").first { it.first == "patch-pane-screen.json" }.second
        val msg = ContractJson.decodeFromJsonElement(DaemonMsg.serializer(), patch) as DaemonMsg.Patch
        val patched = applyPatch(snap, msg.ops, msg.to)
        val node = ContractJson.decodeFromJsonElement(Node.serializer(), patched) as Node.Pane
        assertEquals(10L, node.rev)
        assertEquals("$ ls", node.screen.lines[2])
        assertEquals(4, node.screen.cursor.col)
    }

    /** The result of `open_conversation` in the fixture is what [Opened] reads, and writes back as it was. */
    @Test
    fun theReplyOfOpenConversationIsANodeAndASession() {
        val reply = fixtures("daemon").first { it.first == "reply-ok-open-conversation.json" }.second
        val result = (ContractJson.decodeFromJsonElement(DaemonMsg.serializer(), reply) as DaemonMsg.Reply).result!!
        val opened = ContractJson.decodeFromJsonElement(Opened.serializer(), result)
        assertEquals(36, opened.session.length)
        assertEquals(result, ContractJson.encodeToJsonElement(Opened.serializer(), opened))
    }

    /** The result of `list_conversations` in the fixture is what [ConversationList] reads, and writes back as it was. */
    @Test
    fun theReplyOfListConversationsIsTheSavedConversationsNewestFirst() {
        val reply = fixtures("daemon").first { it.first == "reply-ok-list-conversations.json" }.second
        val result = (ContractJson.decodeFromJsonElement(DaemonMsg.serializer(), reply) as DaemonMsg.Reply).result!!
        val list = ContractJson.decodeFromJsonElement(ConversationList.serializer(), result)
        assertEquals(2, list.conversations.size)
        assertEquals(true, list.conversations[0].at > list.conversations[1].at)
        assertEquals(result, ContractJson.encodeToJsonElement(ConversationList.serializer(), list))
    }

    /** The result of `read_markdown` in the fixture is what [MarkdownDoc] reads, with every kind of block, and writes back as it was. */
    @Test
    fun theReplyOfReadMarkdownIsADocumentOfBlocks() {
        val reply = fixtures("daemon").first { it.first == "reply-ok-read-markdown.json" }.second
        val result = (ContractJson.decodeFromJsonElement(DaemonMsg.serializer(), reply) as DaemonMsg.Reply).result!!
        val doc = ContractJson.decodeFromJsonElement(MarkdownDoc.serializer(), result)
        assertEquals("Plan: pruebas del módulo auth", doc.title)
        assertEquals(
            listOf("Heading", "Para", "Quote", "ListOf", "Code", "Table", "Rule"),
            doc.blocks.map { it::class.simpleName },
        )
        assertEquals(result, ContractJson.encodeToJsonElement(MarkdownDoc.serializer(), doc))
    }

    /** The result of `project_config` in the fixture is what [ProjectInfo] reads, and writes back as it was. */
    @Test
    fun theReplyOfProjectConfigIsAProjectInfo() {
        val reply = fixtures("daemon").first { it.first == "reply-ok-project-config.json" }.second
        val result = (ContractJson.decodeFromJsonElement(DaemonMsg.serializer(), reply) as DaemonMsg.Reply).result!!
        val info = ContractJson.decodeFromJsonElement(ProjectInfo.serializer(), result)
        assertEquals(OpenKind.Claude, info.open)
        assertEquals("../app-trees", info.rootText)
        assertEquals(ConfigFrom.Repo, info.from)
        assertEquals(listOf(".env", "apps/web/.env.local"), info.copy)
        assertEquals(mapOf("web" to 0, "api" to 1), info.ports)
        assertEquals("/home/u/.config/jwn/projects/github.com_acme_app.toml", info.personal)
        assertEquals(result, ContractJson.encodeToJsonElement(ProjectInfo.serializer(), info))
    }
}
