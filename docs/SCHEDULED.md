# Scheduled tasks

Written 2026-11-14 (Scheduled v2). Plan: `MEMORY-AND-SCHEDULE-PLAN.md` section 3 and `VM-MIGRATION-PLAN.md` sections 1 and 2 (phase 1). This page is for people; the last part is for builders.

## What a scheduled task is

Something Zohara Agent does later, once or on a repeat. You make one on the **Scheduled** page (desktop, web, phone) with **New task**, or by asking in a chat: "remind me tomorrow at 9 to call mom", "every weekday at 8, check my disk space and tell me if it is above 90%".

There are three kinds:

| Kind | What happens at the time | Needs a model |
|---|---|---|
| **Task** | Zohara Agent does what you wrote, with its tools, in a normal chat called "Scheduled: <name>". Your permission mode applies (never Bypass for a task on the server). | yes |
| **Reminder** | A notification with your text. Nothing else. It leaves no chat and costs nothing. | no |
| **Browser task** | Like a task, and it may use the browser and web tools (open pages, read, fill in, collect), then reports in a chat. It **asks first** for everything that changes something, and never buys, pays, sends, posts or deletes without your approval. | yes |

## Saying when

Plain words work, and a time with repeat words sets the repeat for you:

* "tomorrow 9am", "next monday", "2026-11-20 18:30"
* "in 30 minutes", "in 2 hours", "in 3 days", "in an hour"
* "every day at 07:30", "every weekday at 9", "every monday at 8"
* "on the 1st of each month", "on the 15th of every month at 9" (a task kept for the 31st runs on the last day of a shorter month)
* Repeat choices: once, every hour, every day, every weekday, every week, every month, or your own ("every 3 days", "every 90 minutes", "every 2 months"; at least 5 minutes, and at least 60 minutes for an account that is not the owner's on the server).

Before you save, the form shows **next runs: Sat 14 Nov 09:00 · Mon 16 Nov 09:00 · ...** (the next three), so you can check the time without running anything. Times are in your own time zone (the phone and the web page send theirs, so "9am" is your 9am even though the server keeps UTC).

## The Scheduled page

Each task shows its name, when it runs next, how often, its kind, where it runs, and **how the last run went** (ok, failed or skipped, when, and what it said). A failure says why ("No model is chosen", "The machine \"vm-1\" is not added", "The daily limit of runs on the server is reached").

* **On/off switch.** An off task does not run; switching it on again moves it to its next time.
* **Run now.** Runs it once now, in the background. The result shows in the card and as a notification. It does not use up the planned time of a repeating task.
* **Test.** Runs it once now in *test mode* and shows what it does as it goes, then a result card: success or failed, the time it took, the tools it used, and what it said. A test is a normal chat called "Test: <name>". It never changes the task's schedule, last run or history. Anything that changes something **still asks first**, even if your mode is Auto or Bypass (a test is never Bypass). A reminder's test shows the notification it would send instead of sending it. The form has a Test button too, so you can try a task before you save it.
* **History.** The last 20 runs, each with its result, how long it took, whether it was "Run now" or on schedule, and a link to its chat.
* **Edit.** Change the text, name, time, repeat, kind or place. Leave the time empty to keep it.

## Where a task runs ("Run on")

| Run on | Needs | Notes |
|---|---|---|
| **This computer** (desktop app) | Zohara Agent running on the computer (it can run in the tray) | The default in the desktop app. Shows the next run and "missed" when the computer was off. |
| **Cloud server** (web and phone) | nothing of yours | The task runs on the Zohara Agent server, even when your computer and phone are off. It uses **your own API key** (the same one your chats use). |
| **An added machine** (Settings > Machines) | the host that owns the task | Phase 1: no program is installed on the machine. The desktop app (or, for the owner, the cloud server) runs the task and the agent works on the machine through the SSH tools: "runs from this computer on vm-1". The host has to be on. |

**Missed runs.** If the computer (or the server) was off at the planned time, the task runs **once** when it is back, and says so ("Missed Sat 14 Nov 09:00"). It is not run again for every missed time.

**What the cloud server allows.** Everybody who is signed in can have scheduled tasks on the server, with limits per person:

| | Owner (administrator) | Everybody else (restricted) |
|---|---|---|
| Kinds | task, reminder, browser task | reminder and task (a task there has only the web search, page reading and memory tools: no shell, files or SSH) |
| How many | 30 | 5 |
| Shortest repeat | 5 minutes | 60 minutes |
| Runs a day | 200 | 24 |
| Machines (SSH) | yes | no |

Nobody can answer an approval on the server while it runs a task alone, so on the server **every approval is answered "no"**: a task that needs to change something there reports what it would have done. The operator can switch cloud tasks to the owner only or off (`ZO_CLOUD_TASKS=admins|off`, default `all`).

## Notifications

When a task finishes you get a notification with the result in one line; a failure says why; a reminder shows its text.

* **Desktop**: a desktop notification (tap to bring the window forward).
* **Web**: a push to your browser when the page is closed or in the background.
* **Phone**: a push to the app (the "Answers" switch in Settings > Notifications decides whether it shows).

For privacy, the server sends the *text* of a notification (the task's name and the result) only to a device whose owner switched on "Show the chat title in notifications"; otherwise the notification only says "A scheduled task finished" (or failed, or "A reminder is due"). Nothing else leaves the server, and a person's notifications only go to that person's own devices.

## Examples

* Reminder: "every weekday at 9, remind me to stand up" (kind Reminder).
* Task: "every day at 07:30, search the web for Rust news and give me three headlines".
* Task on a machine: "every night at 2, check the disk of vm-1 and tell me if it is above 90%" (Run on: vm-1).
* Browser task: "every Monday at 8, open my bank's public rates page and tell me if the rate changed" (it will ask before it clicks anything that changes something).

## For builders

Code:

* `crates/zo-core/src/store/schedules.rs`: the task list, time words (`parse_schedule`, `next_runs`, `next_after`), repeats, history (`Run`, 20 kept), catch-up (`missed_time`, 5 minutes late counts as missed).
* `crates/zo-core/src/scheduler.rs`: the one runner (`execute`, `run_and_record`, `run_due`, `run_now_in_background`), modes (`mode_for`), cloud policy and limits (`cloud_policy`, `limits`, `check_for_host`), the notifier registration (`set_notifier`, `notify`).
* `crates/zo-core/src/service/sched.rs`: the service ops. `crates/zo-server/src/cloudrun.rs`: the cloud runner. `crates/zohara-agent/src/scheduler.rs` (timer) and `sched_ui.rs` (page). Web: `SCHEDULED-BEGIN` block in `crates/zo-server/web/index.html`. Android: `pages/ScheduledScreen.kt`, `Holders.kt`, `ZoApi.kt`.

Service ops (POST `/api/op`, same on the web server, the phone and the desktop's local socket). Every op is for the caller's own tasks only.

| Op | Fields | Answer |
|---|---|---|
| `scheduled` | | `{tasks:[task], host:"cloud"\|"desktop", run_on:[{id,label,note}], kinds:[...], limits:{max_tasks,min_repeat_minutes,runs_per_day,runs_today}, now}` |
| `schedule_add` | `task` (or `prompt`), `when`, optional `repeat`, `title`, `kind` (task\|reminder\|browser), `run_on` ("", "this", "cloud", "machine:<name>"), `tz` (IANA), `mode` (auto\|manual), `project` | `{task, message}` |
| `schedule_update` | `id` plus any of the fields above; `when` omitted keeps the time | `{task}` |
| `schedule_enable` | `id`, `on` | `{task}` |
| `schedule_run_now` | `id` | `{started:true, task}`; the result arrives in the task (`last_*`, history) and as a notification |
| `schedule_history` | `id` | `{task, history:[{time,status,summary,chat,ms,missed,how}]}` newest first |
| `schedule_preview` | `when`, optional `repeat`, `tz` | `{repeat, runs:[{at,label}], text:"next run: ..."}` or an error sentence |
| `schedule_cancel` | `id` | `{}` |
| `schedule_test` | `id` of a saved task, or `draft:{task,kind,title,run_on,tz,mode}` | **streamed** (below) |

A task: `{id,title,prompt,next_run,repeat,enabled,project,created,last_run,last_chat,kind,mode,run_on,tz,day,last_status,last_output,history_count,running,place,runs_here}`. Fields the older server did not have are optional on every client.

`schedule_test` streams NDJSON like `send`. On the web server it is `POST /api/schedule/test` (same sign-in, same place among the running answers, approvals and Stop through `/api/reply` with the `stream` number): `{"event":"stream","id":N}`, `test` (title, kind, place), then `delta`, `text`, `tool_start`, `tool_done`, `approval` (answer it with `/api/reply {stream, op:"approve", id, yes}`), `usage`, `notify` (title, body, kind, ok: what a reminder would show), `result` (`{ok,status,summary,tools,ms,chat}`), `done`; mistakes arrive as `error` then `done`. Closing the connection stops the run.

**Cloud runner** (`cloudrun.rs`): the server keeps a small index (in memory and in the Firestore collection `sched_index`, one document per person with the next due time and the admin flag; no task text) so it can find who is due without reading every person's folder. Every 10 seconds the people who are due get one thread each: their data is loaded, one place of `ZO_MAX_RUNS` is held, `run_due` runs what is due with `approval::Deny`, and the person's devices get a push (`push::TaskNote`). The index is refreshed after every `schedule*` op, after a run, and dropped when the account is deleted. A person's data is only touched inside their `tenant::scope`.

Not built yet (see the plan documents): the headless runner for VMs (`zohara-agent-runner`), managing a desktop's tasks from the web through the desktop link, and the Machines popup.
