package dev.jw.next.contract

import kotlinx.serialization.json.Json
import kotlinx.serialization.json.JsonArray
import kotlinx.serialization.json.JsonObject
import kotlinx.serialization.json.JsonPrimitive
import kotlinx.serialization.json.jsonArray
import kotlin.test.Test
import kotlin.test.assertEquals

/** REQ-271: the daemon says a list that lost its first items as removals at 0; the phone applies them. */
class PatchFrontTest {
    private fun doc(vararg ids: Int) = Json.parseToJsonElement(
        """{"rev": 1, "blocks": [${ids.joinToString(",") { """{"id": $it}""" }}]}""",
    )

    @Test
    fun theFirstItemsOfAListGoAndTheRestShiftDown() {
        val ops = listOf(
            PatchOp.Remove("/blocks/0"),
            PatchOp.Replace("/blocks/1/id", JsonPrimitive(30)),
            PatchOp.Add("/blocks/-", Json.parseToJsonElement("""{"id": 4}""")),
        )
        val out = applyPatch(doc(1, 2, 3), ops, 2)
        val ids = (out["blocks"] as JsonArray).map { ((it as JsonObject)["id"] as JsonPrimitive).content }
        assertEquals(listOf("2", "30", "4"), ids)
        assertEquals("2", (out["rev"] as JsonPrimitive).content)
    }

    @Test
    fun severalFirstItemsAreRemovedOneAfterTheOther() {
        val out = applyPatch(doc(1, 2, 3, 4), List(2) { PatchOp.Remove("/blocks/0") }, 5)
        assertEquals(2, out["blocks"]!!.jsonArray.size)
    }
}
