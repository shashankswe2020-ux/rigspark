# RigSpark Community Terms of Use

Last updated: 9 November 2026

These terms apply to RigSpark Community: the `rigspark` command-line tool, the
browser interface it serves on your computer, and the RigSpark desktop app
(together, "RigSpark"). By installing, launching or using RigSpark you agree to
them. If you do not agree, do not use RigSpark.

## 1. What RigSpark is

RigSpark is software that runs on your own computer. It checks your hardware,
recommends local AI models that fit it, downloads and verifies model files you
choose, and starts local model servers so you can chat, generate images and
video, and work with speech. RigSpark does not run models for you on remote
servers, and it is not a hosted service.

## 2. Who may use RigSpark

You may use RigSpark if you can form a binding agreement where you live. If you
use RigSpark for an organisation, you confirm that you may accept these terms on
its behalf, and "you" includes that organisation.

## 3. Models and their licences

RigSpark helps you find and download models made by third parties. RigSpark does
not own those models and does not grant you any rights in them.

- Each model has its own licence, set by its author. RigSpark shows the licence
  recorded in its catalog before you download, but you are responsible for
  reading the licence and complying with it, including any limits on commercial
  use, number of users, field of use or how outputs may be used.
- Catalog information (sizes, licences, speed ranges, fit verdicts) is provided
  for convenience. It can be incomplete or out of date. Where RigSpark cannot
  source a figure, it shows `unknown` rather than an estimate.
- Fit verdicts are estimates based on your hardware and published model sizes.
  A model that RigSpark says fits may still fail to load or run slowly, and one
  it says will not fit may still run if you choose to override the check.

## 4. Your content and outputs

You keep whatever rights you have in the prompts, files and other material you
provide, and in the outputs you generate, subject to the licence of the model
that produced them.

You are responsible for what you generate and how you use it. That includes
text, images, video and speech; material that depicts or imitates real people,
their likeness or their voice; and material protected by copyright, trademark
or other rights. Model output can be inaccurate, biased or offensive. Check it
before you rely on it, publish it or act on it.

## 5. Acceptable use

You agree not to use RigSpark, or anything you make with it, to:

- break any law or regulation that applies to you;
- infringe or misappropriate anyone's intellectual property, privacy or
  publicity rights;
- create or share sexual content involving minors, or non-consensual intimate
  imagery of real people;
- harass, threaten, defraud or impersonate any person or organisation, or create
  deceptive content intended to mislead people about who made it;
- develop malware, attack computer systems, or gain unauthorised access to data
  or networks;
- violate the licence or acceptable-use policy of any model you run.

## 6. Hardware and resource use

Running local models is demanding. Depending on the model and settings,
RigSpark and the runtimes it starts can use most of your computer's memory,
processor and graphics capacity for long periods, write large files to disk,
raise temperatures and fan noise, and drain a laptop battery quickly. Model
downloads can be tens of gigabytes and may count against a metered or capped
internet plan.

You choose which models to download and start, and you can override fit checks.
You are responsible for making sure your hardware, power, cooling, storage and
network can handle that use, and for stopping a model that makes your computer
unstable. RigSpark stops the servers it started when you ask it to, but it
cannot guarantee that a third-party runtime releases every resource.

## 7. Privacy and local data

RigSpark is designed to keep your data on your computer.

- Chats, memory, files you attach, generated media, settings and model files are
  stored on your device, mainly in the `.rigspark` folder in your home directory
  and in folders you choose.
- RigSpark's servers listen only on the loopback address (127.0.0.1) and are not
  reachable from other computers unless you change that yourself.
- RigSpark does not collect usage analytics or send your prompts, files or
  outputs to its authors.

You are responsible for securing your device and backing up your data. Deleting
the `.rigspark` folder removes RigSpark's stored data from your computer.

## 8. Network access and third-party services

RigSpark connects to the internet only to do things you ask for or that keep it
working:

- downloading model files from Hugging Face, the Ollama registry and other
  sources listed in its catalog;
- downloading catalog updates and checking for new RigSpark releases on GitHub;
- reaching cloud model providers, coding agents and connectors (MCP servers)
  that you configure.

Those services are run by third parties under their own terms and privacy
policies. Data you send to them, for example through a connector or a cloud
model, is governed by their terms, not these. RigSpark's authors do not control
and are not responsible for third-party services, including their availability,
content, pricing or changes.

Every connector tool call that can change something outside your computer asks
for your approval first. You are responsible for the calls you approve.

## 9. Updates and the catalog

RigSpark's model catalog and the software itself may change over time. Updates
can add, change or remove features, models and catalog entries. RigSpark checks
the integrity of catalog updates and model downloads before using them, but it
cannot guarantee that a third-party model is safe, accurate or fit for your
purpose.

## 10. Experimental features

Some features are marked as previews or experiments, or depend on third-party
tools that are still changing (for example image, video and speech engines).
They may change or be removed without notice, may behave differently on
different hardware, and may produce lower-quality or unexpected results. Use
them with the same care as any other model output.

## 11. Feedback and contributions

You are welcome to report problems, suggest features and contribute code. If you
send feedback, the RigSpark authors may use it without obligation to you.
Contributions to the RigSpark repository are accepted under the terms described
in `CONTRIBUTING.md`, including the licence that applies to contributed code and
the sign-off each commit needs.

## 12. Children

RigSpark is not directed at children. If you are under the age of digital
consent where you live, use RigSpark only with the involvement of a parent or
guardian, and only with models and content suitable for your age.

## 13. Names and logos

"RigSpark", the RigSpark logo, the mascot and related names and marks identify
the official RigSpark project. They are not licensed to you for reuse. If you
distribute a modified version of RigSpark, give it a different name and logo,
and do not present it as an official RigSpark release or imply that the
RigSpark authors endorse it.

## 14. Source code licence and third-party components

RigSpark Community's source code is distributed under the MIT licence; its full
text is in the `LICENSE` file at
<https://github.com/shashankswe2020-ux/rigspark/blob/main/LICENSE>. Nothing in
these terms limits the rights that licence grants for the code. RigSpark also
includes third-party components (such as fonts and JavaScript libraries) under
their own licences, listed with those components.

A separately licensed edition of RigSpark may be offered in the future under
different terms. These terms do not apply to it.

## 15. No warranty

RIGSPARK IS PROVIDED "AS IS" AND "AS AVAILABLE", WITHOUT WARRANTY OF ANY KIND,
EXPRESS OR IMPLIED, INCLUDING THE WARRANTIES OF MERCHANTABILITY, FITNESS FOR A
PARTICULAR PURPOSE, TITLE AND NON-INFRINGEMENT. THE AUTHORS DO NOT WARRANT THAT
RIGSPARK WILL BE UNINTERRUPTED OR ERROR-FREE, THAT ITS RECOMMENDATIONS OR
ESTIMATES ARE ACCURATE, OR THAT ANY MODEL OR OUTPUT IS SAFE OR SUITABLE FOR YOUR
USE. Some jurisdictions do not allow the exclusion of certain warranties, so
some of these exclusions may not apply to you.

## 16. Limitation of liability

TO THE FULLEST EXTENT PERMITTED BY LAW, THE AUTHORS AND COPYRIGHT HOLDERS OF
RIGSPARK ARE NOT LIABLE FOR ANY CLAIM, DAMAGES OR OTHER LIABILITY, WHETHER IN
CONTRACT, TORT OR OTHERWISE, ARISING FROM OR IN CONNECTION WITH RIGSPARK OR ITS
USE, INCLUDING LOSS OF DATA, LOSS OF PROFITS, HARDWARE DAMAGE OR SYSTEM
INSTABILITY CAUSED BY RUNNING MODELS, OR HARM ARISING FROM MODEL OUTPUTS OR
THIRD-PARTY SERVICES. Where liability cannot be excluded, it is limited to the
amount you paid for RigSpark, which for RigSpark Community is zero.

## 17. Indemnity

To the extent permitted by law, you agree to defend and indemnify RigSpark's
authors against claims, losses and costs (including reasonable legal fees)
arising from your use of RigSpark in breach of these terms or the law, from
content you generate or distribute, or from your violation of a model's licence
or a third party's rights.

## 18. Export and sanctions

You are responsible for complying with export-control and sanctions laws that
apply to you, including restrictions on particular models, countries, entities
or end uses.

## 19. Changes to these terms

The authors may update these terms. The date at the top shows the latest
version. When the terms change in a way that matters, RigSpark shows the
first-run notice again. If you keep using RigSpark after an update, you accept
the updated terms; if you do not accept them, stop using RigSpark.

## 20. Ending these terms

You can stop using RigSpark at any time by uninstalling it and deleting its data.
These terms stop applying to you when you do, except sections 4, 11 and 13 to 17, 20 and
21, which continue to apply.

## 21. Governing law and contact

These terms are governed by the laws of [JURISDICTION — to be confirmed by
counsel], without regard to conflict-of-laws rules, and disputes will be heard
in the courts of [VENUE — to be confirmed by counsel], unless the law where you
live requires otherwise.

If a provision is found unenforceable, the rest stays in effect. These terms,
together with the licences referred to above, are the whole agreement between
you and the RigSpark authors about RigSpark Community.

Questions: open an issue at <https://github.com/shashankswe2020-ux/rigspark/issues>
or contact [CONTACT EMAIL — to be added].
