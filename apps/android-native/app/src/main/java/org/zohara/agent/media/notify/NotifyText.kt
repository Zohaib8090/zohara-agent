package org.zohara.agent.media.notify

/** The words of the notifications. Privacy first (NOTIF-11, INTERACTION.md section 3): without "Show details" nothing of the question or the command is in the text. */
object NotifyText {
  class Text(val title: String, val body: String)

  const val APP = "Zohara Agent"

  /** NOTIF-01: "Your answer is ready" under the title Zohara Agent, or with titles on the chat title with "Zohara Agent: your answer is ready" */
  fun answer(showTitle: Boolean, chatTitle: String?): Text {
    val t = clip(chatTitle)
    return if (showTitle && t.isNotEmpty()) Text(t, "Zohara Agent: your answer is ready") else Text(APP, "Your answer is ready")
  }

  /**
   * A scheduled task ended, or a reminder is due (docs/SCHEDULED.md). Without "Show the chat title in notifications" only the fact is said (the server does
   * not send the text then); with it the task's name is the title and the short result (or the reminder's text) the body.
   */
  fun task(showTitle: Boolean, title: String?, body: String?, ok: Boolean, reminder: Boolean): Text {
    val fact = if (reminder) "A reminder is due" else if (ok) "A scheduled task finished" else "A scheduled task failed"
    val t = clip(title)
    val b = clip(body, 140)
    return if (showTitle && t.isNotEmpty()) Text(t, b.ifEmpty { fact }) else Text(APP, fact)
  }

  /** an approval: default "Zohara Agent needs your decision"; with details the summary */
  fun approval(details: Boolean, summary: String?): Text {
    val s = clip(summary, 120)
    return if (details && s.isNotEmpty()) Text("Zohara Agent needs you", s) else Text(APP, "Zohara Agent needs your decision")
  }

  /** a question: default "Zohara Agent needs your decision"; with details the first question */
  fun question(details: Boolean, firstQuestion: String?): Text {
    val s = clip(firstQuestion, 120)
    return if (details && s.isNotEmpty()) Text("Zohara Agent needs you", s) else Text(APP, "Zohara Agent needs your decision")
  }

  /** control characters out, at most [max] characters */
  fun clip(s: String?, max: Int = 60): String {
    val c = s.orEmpty().filter { !it.isISOControl() }.trim()
    return if (c.length > max) c.take(max - 1).trimEnd() + "…" else c
  }
}
