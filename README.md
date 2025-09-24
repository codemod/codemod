<p align="center">
  <a href="https://codemod.com">
      <picture>
          <source media="(prefers-color-scheme: dark)" srcset="/apps/docs/images/intro/codemod-docs-hero-dark.jpg">
          <source media="(prefers-color-scheme: light)" srcset="/apps/docs/images/intro/codemod-docs-hero-light.jpg">
          <img alt="Codemod Registry Header" src="/apps/docs/images/intro/codemod-docs-hero-light.jpg"></picture>
  </a>

  <p align="center">
    <br />
    <a href="https://go.codemod.com/app">Platform</a>
    ·
    <a href="https://codemod.com/community">Community</a>
    ·
    <a href="https://docs.codemod.com">Docs</a>
  </p>
</p>

# Codemod

[![Community](https://img.shields.io/badge/slack-join-e9a820)](https://codemod.com/community)
[![License](https://img.shields.io/github/license/codemod-com/codemod)](LICENSE)

Codemod is the AI-powered, community-led platform for automating code migrations, framework upgrades, and large-scale changes.

This repo houses the **Codemod CLI** and its underlying **workflow engine** – everything you need to scaffold, test, and run codemods locally or in CI. The CLI can also connect to Codemod’s hosted Registry and Platform when you want to publish or orchestrate migrations across many repositories.

## Key Features

- **Workflows** – Scaffold, test, and orchestrate complex, multi-step migrations with ast-grep YAML or JavaScipt ast-grep (jssg).
- **Codemod Registry** – Share or discover community codemods via `npx codemod publish` or `npx codemod search`.

## Getting Started

```bash
# 1) Scaffold a new codemod project
npx codemod init my-codemod

# 2) Test it locally
npx codemod workflow run -w my-codemod/workflow.yaml

# 3) Publish it when you're ready
npx codemod publish my-codemod

# 4) Run it from the Registry
npx codemod @codemod-com/my-codemod
```

See the full [CLI reference](https://docs.codemod.com/cli/cli-reference) for every command and option.

## Codemod Platform

The CLI is the engine that powers campaigns inside [Codemod Platform](https://app.codemod.com).
Need to coordinate changes across multiple repositories, track progress, or collaborate with your team? The Platform adds insights, orchestration, dashboards, and authoring tools that help teams successfully land large-scale code migrations.

Learn more in the [docs](https://docs.codemod.com) and share feedback on the [feedback board](https://feedback.codemod.com). We also hang out in Slack — join us at [go.codemod.com/community](https://go.codemod.com/community).

## Contributing

We welcome contributions to the Codemod CLI. Please see the [contributing guide](/CONTRIBUTING.md) to get started.

### Contributors ✨

![Alt](https://repobeats.axiom.co/api/embed/a601b01213da7b870268b1fe2ee8384efc40c308.svg "Repobeats analytics image")

Thanks goes to these wonderful people ([emoji key](https://allcontributors.org/docs/en/emoji-key)):

<!-- ALL-CONTRIBUTORS-LIST:START - Do not remove or modify this section -->
<!-- prettier-ignore-start -->
<!-- markdownlint-disable -->
<table>
  <tbody>
    <tr>
      <td align="center" valign="top" width="14.28%"><a href="https://github.com/alexbit-codemod/"><img src="https://avatars.githubusercontent.com/u/78109534?v=4?s=100" width="100px;" alt="Alex Bit"/><br /><sub><b>Alex Bit</b></sub></a><br /><a href="https://github.com/codemod-com/codemod/commits?author=alexbit-codemod" title="Code">💻</a> <a href="https://github.com/codemod-com/codemod/commits?author=alexbit-codemod" title="Documentation">📖</a></td>
      <td align="center" valign="top" width="14.28%"><a href="https://github.com/mohebifar"><img src="https://avatars.githubusercontent.com/u/6104558?v=4?s=100" width="100px;" alt="Mohamad Mohebifar"/><br /><sub><b>Mohamad Mohebifar</b></sub></a><br /><a href="https://github.com/codemod-com/codemod/commits?author=mohebifar" title="Code">💻</a> <a href="https://github.com/codemod-com/codemod/issues?q=author%3Amohebifar" title="Bug reports">🐛</a></td>
      <td align="center" valign="top" width="14.28%"><a href="https://github.com/amirabbas-gh"><img src="https://avatars.githubusercontent.com/u/93479436?v=4?s=100" width="100px;" alt="amirabbas-gh"/><br /><sub><b>Amirabbas Ghasemi</b></sub></a><br /><a href="https://github.com/codemod-com/codemod/commits?author=amirabbas-gh" title="Code">💻</a> <a href="https://github.com/codemod-com/codemod/issues?q=author%3Aamirabbas-gh" title="Bug reports">🐛</a></td>
      <td align="center" valign="top" width="14.28%"><a href="https://github.com/mohab-sameh"><img src="https://avatars.githubusercontent.com/u/37941642?v=4?s=100" width="100px;" alt="Mohab Sameh"/><br /><sub><b>Mohab Sameh</b></sub></a><br /><a href="https://github.com/codemod-com/codemod/commits?author=mohab-sameh" title="Code">💻</a> <a href="https://github.com/codemod-com/codemod/commits?author=mohab-sameh" title="Documentation">📖</a></td>
      <td align="center" valign="top" width="14.28%"><a href="https://github.com/siamak"><img src="https://avatars.githubusercontent.com/u/3109082?v=4?s=100" width="100px;" alt="siamak"/><br /><sub><b>Siamak</b></sub></a><br /><a href="https://github.com/codemod-com/codemod/commits?author=siamak" title="Code">💻</a> <a href="https://github.com/codemod-com/codemod/issues?q=author%3Asiamak" title="Bug reports">🐛</a> <a href="https://github.com/codemod-com/codemod/commits?author=siamak" title="Design">🎨</a></td>
      <td align="center" valign="top" width="14.28%"><a href="http://jahred.me"><img src="https://avatars.githubusercontent.com/u/13903378?v=4?s=100" width="100px;" alt="Jahred Hope"/><br /><sub><b>Jahred Hope</b></sub></a><br /><a href="https://github.com/codemod-com/codemod/commits?author=jahredhope" title="Code">💻</a></td>
      <td align="center" valign="top" width="14.28%"><a href="https://github.com/JakobJingleheimer"><img src="https://avatars.githubusercontent.com/u/3012099?v=4?s=100" width="100px;" alt="Jacob Smith"/><br /><sub><b>Jacob Smith</b></sub></a><br /><a href="https://github.com/codemod-com/codemod/commits?author=JakobJingleheimer" title="Documentation">📖</a></td>
    </tr>
    <tr>
      <td align="center" valign="top" width="14.28%"><a href="http://augustinmauroy.github.io/"><img src="https://avatars.githubusercontent.com/u/97875033?v=4?s=100" width="100px;" alt="Augustin Mauroy"/><br /><sub><b>Augustin Mauroy</b></sub></a><br /><a href="https://github.com/codemod-com/codemod/commits?author=AugustinMauroy" title="Code">💻</a> <a href="https://github.com/codemod-com/codemod/issues?q=author%3AAugustinMauroy" title="Bug reports">🐛</a> <a href="#ideas-AugustinMauroy" title="Ideas, Planning, & Feedback">🤔</a></td>
      <td align="center" valign="top" width="14.28%"><a href="http://brunocroh.com"><img src="https://avatars.githubusercontent.com/u/13812512?v=4?s=100" width="100px;" alt="Bruno Rodrigues"/><br /><sub><b>Bruno Rodrigues</b></sub></a><br /><a href="https://github.com/codemod-com/codemod/commits?author=brunocroh" title="Code">💻</a></td>
      <td align="center" valign="top" width="14.28%"><a href="https://github.com/spirulence"><img src="https://avatars.githubusercontent.com/u/1297151?v=4?s=100" width="100px;" alt="Cameron Seebach"/><br /><sub><b>Cameron Seebach</b></sub></a><br /><a href="https://github.com/codemod-com/codemod/issues?q=author%3Aspirulence" title="Bug reports">🐛</a> <a href="https://github.com/codemod-com/codemod/commits?author=spirulence" title="Code">💻</a></td>
      <td align="center" valign="top" width="14.28%"><a href="https://github.com/hbjORbj"><img src="https://avatars.githubusercontent.com/u/32841130?v=4?s=100" width="100px;" alt="Benny Joo"/><br /><sub><b>Benny Joo</b></sub></a><br /><a href="https://github.com/codemod-com/codemod/commits?author=hbjORbj" title="Code">💻</a> <a href="https://github.com/codemod-com/codemod/issues?q=author%3AhbjORbj" title="Bug reports">🐛</a> <a href="https://github.com/codemod-com/codemod/commits?author=hbjORbj" title="Documentation">📖</a></td>
      <td align="center" valign="top" width="14.28%"><a href="https://github.com/grzim"><img src="https://avatars.githubusercontent.com/u/8398557?v=4?s=100" width="100px;" alt="Grzegorz"/><br /><sub><b>Grzegorz</b></sub></a><br /><a href="https://github.com/codemod-com/codemod/issues?q=author%3Agrzim" title="Bug reports">🐛</a> <a href="https://github.com/codemod-com/codemod/commits?author=grzim" title="Code">💻</a></td>
      <td align="center" valign="top" width="14.28%"><a href="https://medium.com/@greg-pabian/"><img src="https://avatars.githubusercontent.com/u/35925521?v=4?s=100" width="100px;" alt="Greg Pabian"/><br /><sub><b>Greg Pabian</b></sub></a><br /><a href="https://github.com/codemod-com/codemod/commits?author=grzpab" title="Code">💻</a> <a href="https://github.com/codemod-com/codemod/issues?q=author%3Agrzpab" title="Bug reports">🐛</a> <a href="https://github.com/codemod-com/codemod/commits?author=grzpab" title="Documentation">📖</a></td>
      <td align="center" valign="top" width="14.28%"><a href="https://github.com/arshcodemod"><img src="https://avatars.githubusercontent.com/u/167029627?v=4?s=100" width="100px;" alt="arshcodemod"/><br /><sub><b>arshcodemod</b></sub></a><br /><a href="https://github.com/codemod-com/codemod/commits?author=arshcodemod" title="Documentation">📖</a> <a href="#talk-arshcodemod" title="Talks">📢</a></td>
    </tr>
    <tr>
      <td align="center" valign="top" width="14.28%"><a href="http://r4zen.dev"><img src="https://avatars.githubusercontent.com/u/44036750?v=4?s=100" width="100px;" alt="Sasha"/><br /><sub><b>Sasha</b></sub></a><br /><a href="https://github.com/codemod-com/codemod/commits?author=r4zendev" title="Code">💻</a> <a href="https://github.com/codemod-com/codemod/issues?q=author%3Ar4zendev" title="Bug reports">🐛</a></td>
      <td align="center" valign="top" width="14.28%"><a href="http://demidoff.dev"><img src="https://avatars.githubusercontent.com/u/12803556?v=4?s=100" width="100px;" alt="Mikhail Demidoff"/><br /><sub><b>Mikhail Demidoff</b></sub></a><br /><a href="https://github.com/codemod-com/codemod/commits?author=mirai2k" title="Code">💻</a></td>
      <td align="center" valign="top" width="14.28%"><a href="https://github.com/arybitskiy"><img src="https://avatars.githubusercontent.com/u/4195805?v=4?s=100" width="100px;" alt="Aleksy"/><br /><sub><b>Aleksy</b></sub></a><br /><a href="https://github.com/codemod-com/codemod/commits?author=arybitskiy" title="Code">💻</a> <a href="https://github.com/codemod-com/codemod/issues?q=author%3Aarybitskiy" title="Bug reports">🐛</a></td>
      <td align="center" valign="top" width="14.28%"><a href="http://pardisp.github.io"><img src="https://avatars.githubusercontent.com/u/8430554?v=4?s=100" width="100px;" alt="Pardis Pashakhanloo"/><br /><sub><b>Pardis Pashakhanloo</b></sub></a><br /><a href="https://github.com/codemod-com/codemod/commits?author=ppashakhanloo" title="Code">💻</a> <a href="#blog-ppashakhanloo" title="Blogposts">📝</a> <a href="#data-ppashakhanloo" title="Data">🔣</a></td>
      <td align="center" valign="top" width="14.28%"><a href="https://github.com/DmytroHryshyn"><img src="https://avatars.githubusercontent.com/u/125881252?v=4?s=100" width="100px;" alt="DmytroHryshyn"/><br /><sub><b>DmytroHryshyn</b></sub></a><br /><a href="https://github.com/codemod-com/codemod/commits?author=DmytroHryshyn" title="Code">💻</a> <a href="https://github.com/codemod-com/codemod/issues?q=author%3ADmytroHryshyn" title="Bug reports">🐛</a></td>
      <td align="center" valign="top" width="14.28%"><a href="https://github.com/kevtran2"><img src="https://avatars.githubusercontent.com/u/44513934?v=4?s=100" width="100px;" alt="kevtran2"/><br /><sub><b>kevtran2</b></sub></a><br /><a href="https://github.com/codemod-com/codemod/commits?author=kevtran2" title="Documentation">📖</a> <a href="https://github.com/codemod-com/codemod/commits?author=kevtran2" title="Code">💻</a></td>
      <td align="center" valign="top" width="14.28%"><a href="https://maxleiter.com/"><img src="https://avatars.githubusercontent.com/u/8675906?v=4?s=100" width="100px;" alt="Max Leiter"/><br /><sub><b>Max Leiter</b></sub></a><br /><a href="https://github.com/codemod-com/codemod/commits?author=MaxLeiter" title="Code">💻</a></td>
    </tr>
    <tr>
      <td align="center" valign="top" width="14.28%"><a href="https://github.com/manishjha-04"><img src="https://avatars.githubusercontent.com/u/128405481?v=4?s=100" width="100px;" alt="Manish Jha"/><br /><sub><b>Manish Jha</b></sub></a><br /><a href="https://github.com/codemod-com/codemod/issues?q=author%3Amanishjha-04" title="Bug reports">🐛</a> <a href="https://github.com/codemod-com/codemod/commits?author=manishjha-04" title="Code">💻</a></td>
      <td align="center" valign="top" width="14.28%"><a href="https://with-heart.xyz/"><img src="https://avatars.githubusercontent.com/u/1954752?v=4?s=100" width="100px;" alt="with-heart"/><br /><sub><b>with-heart</b></sub></a><br /><a href="https://github.com/codemod-com/codemod/issues?q=author%3Awith-heart" title="Bug reports">🐛</a> <a href="https://github.com/codemod-com/codemod/commits?author=with-heart" title="Documentation">📖</a> <a href="#ideas-with-heart" title="Ideas, Planning, & Feedback">🤔</a></td>
      <td align="center" valign="top" width="14.28%"><a href="https://github.com/AnielloFalcone"><img src="https://avatars.githubusercontent.com/u/16816912?v=4?s=100" width="100px;" alt="Aniello Falcone"/><br /><sub><b>Aniello Falcone</b></sub></a><br /><a href="https://github.com/codemod-com/codemod/issues?q=author%3AAnielloFalcone" title="Bug reports">🐛</a></td>
      <td align="center" valign="top" width="14.28%"><a href="https://www.linkedin.com/in/harshgupta749"><img src="https://avatars.githubusercontent.com/u/22460730?v=4?s=100" width="100px;" alt="Harsh Gupta"/><br /><sub><b>Harsh Gupta</b></sub></a><br /><a href="https://github.com/codemod-com/codemod/commits?author=Harsh24893" title="Code">💻</a></td>
      <td align="center" valign="top" width="14.28%"><a href="https://github.com/Nuurek"><img src="https://avatars.githubusercontent.com/u/12049357?v=4?s=100" width="100px;" alt="Nuurek"/><br /><sub><b>Nuurek</b></sub></a><br /><a href="https://github.com/codemod-com/codemod/commits?author=Nuurek" title="Code">💻</a> <a href="https://github.com/codemod-com/codemod/issues?q=author%3ANuurek" title="Bug reports">🐛</a></td>
      <td align="center" valign="top" width="14.28%"><a href="https://github.com/Baghdasarov"><img src="https://avatars.githubusercontent.com/u/16937233?v=4?s=100" width="100px;" alt="Norayr Baghdasarov"/><br /><sub><b>Norayr Baghdasarov</b></sub></a><br /><a href="https://github.com/codemod-com/codemod/commits?author=Baghdasarov" title="Code">💻</a> <a href="https://github.com/codemod-com/codemod/issues?q=author%3ABaghdasarov" title="Bug reports">🐛</a></td>
      <td align="center" valign="top" width="14.28%"><a href="https://getstell.com"><img src="https://avatars.githubusercontent.com/u/106824?v=4?s=100" width="100px;" alt="Espen Bye"/><br /><sub><b>Espen Bye</b></sub></a><br /><a href="https://github.com/codemod-com/codemod/issues?q=author%3Aespenbye" title="Bug reports">🐛</a></td>
    </tr>
    <tr>
      <td align="center" valign="top" width="14.28%"><a href="https://jakeriordan.dev"><img src="https://avatars.githubusercontent.com/u/38506433?v=4?s=100" width="100px;" alt="Jake Riordan"/><br /><sub><b>Jake Riordan</b></sub></a><br /><a href="https://github.com/codemod-com/codemod/commits?author=Hazetheai" title="Code">💻</a> <a href="https://github.com/codemod-com/codemod/issues?q=author%3AHazetheai" title="Bug reports">🐛</a></td>
      <td align="center" valign="top" width="14.28%"><a href="http://jonathangiardino.com"><img src="https://avatars.githubusercontent.com/u/47124762?v=4?s=100" width="100px;" alt="Jonathan Giardino"/><br /><sub><b>Jonathan Giardino</b></sub></a><br /><a href="https://github.com/codemod-com/codemod/commits?author=jonathangiardino" title="Code">💻</a> <a href="https://github.com/codemod-com/codemod/issues?q=author%3Ajonathangiardino" title="Bug reports">🐛</a></td>
      <td align="center" valign="top" width="14.28%"><a href="https://github.com/diboune"><img src="https://avatars.githubusercontent.com/u/60890389?v=4?s=100" width="100px;" alt="Mohamed Amine Diboune"/><br /><sub><b>Mohamed Amine Diboune</b></sub></a><br /><a href="https://github.com/codemod-com/codemod/commits?author=diboune" title="Code">💻</a></td>
      <td align="center" valign="top" width="14.28%"><a href="https://github.com/siffogh"><img src="https://avatars.githubusercontent.com/u/4761023?v=4?s=100" width="100px;" alt="Seif Ghezala"/><br /><sub><b>Seif Ghezala</b></sub></a><br /><a href="https://github.com/codemod-com/codemod/commits?author=siffogh" title="Code">💻</a></td>
      <td align="center" valign="top" width="14.28%"><a href="https://www.toptal.com/resume/ahmad-soliman"><img src="https://avatars.githubusercontent.com/u/3511001?v=4?s=100" width="100px;" alt="Ahmad Soliman"/><br /><sub><b>Ahmad Soliman</b></sub></a><br /><a href="https://github.com/codemod-com/codemod/commits?author=ahmadsoliman" title="Code">💻</a></td>
      <td align="center" valign="top" width="14.28%"><a href="https://github.com/ml"><img src="https://avatars.githubusercontent.com/u/1509?v=4?s=100" width="100px;" alt="Maciej Lotkowski"/><br /><sub><b>Maciej Lotkowski</b></sub></a><br /><a href="https://github.com/codemod-com/codemod/issues?q=author%3Aml" title="Bug reports">🐛</a></td>
      <td align="center" valign="top" width="14.28%"><a href="https://github.com/35C4n0r"><img src="https://avatars.githubusercontent.com/u/70096901?v=4?s=100" width="100px;" alt="Jay Kumar"/><br /><sub><b>Jay Kumar</b></sub></a><br /><a href="https://github.com/codemod-com/codemod/commits?author=35C4n0r" title="Code">💻</a></td>
    </tr>
    <tr>
      <td align="center" valign="top" width="14.28%"><a href="https://fujino.dev"><img src="https://avatars.githubusercontent.com/u/7889778?v=4?s=100" width="100px;" alt="Shinya Fujino"/><br /><sub><b>Shinya Fujino</b></sub></a><br /><a href="https://github.com/codemod-com/codemod/commits?author=morinokami" title="Documentation">📖</a> <a href="https://github.com/codemod-com/codemod/commits?author=morinokami" title="Code">💻</a> <a href="https://github.com/codemod-com/codemod/issues?q=author%3Amorinokami" title="Bug reports">🐛</a></td>
      <td align="center" valign="top" width="14.28%"><a href="http://coderaiser.github.io"><img src="https://avatars.githubusercontent.com/u/1573141?v=4?s=100" width="100px;" alt="coderaiser"/><br /><sub><b>coderaiser</b></sub></a><br /><a href="https://github.com/codemod-com/codemod/commits?author=coderaiser" title="Code">💻</a></td>
      <td align="center" valign="top" width="14.28%"><a href="http://zachwolf.com"><img src="https://avatars.githubusercontent.com/u/606237?v=4?s=100" width="100px;" alt="Zach Wolf"/><br /><sub><b>Zach Wolf</b></sub></a><br /><a href="https://github.com/codemod-com/codemod/issues?q=author%3Azachwolf" title="Bug reports">🐛</a></td>
      <td align="center" valign="top" width="14.28%"><a href="https://zergus.github.io/"><img src="https://avatars.githubusercontent.com/u/5468045?v=4?s=100" width="100px;" alt="Serhii Melnyk"/><br /><sub><b>Serhii Melnyk</b></sub></a><br /><a href="#ideas-Zergus" title="Ideas, Planning, & Feedback">🤔</a></td>
      <td align="center" valign="top" width="14.28%"><a href="https://github.com/henryqdineen"><img src="https://avatars.githubusercontent.com/u/682132?v=4?s=100" width="100px;" alt="Henry Q. Dineen"/><br /><sub><b>Henry Q. Dineen</b></sub></a><br /><a href="https://github.com/codemod-com/codemod/commits?author=henryqdineen" title="Code">💻</a> <a href="https://github.com/codemod-com/codemod/issues?q=author%3Ahenryqdineen" title="Bug reports">🐛</a></td>
      <td align="center" valign="top" width="14.28%"><a href="https://blog.xmatthias.com"><img src="https://avatars.githubusercontent.com/u/5024695?v=4?s=100" width="100px;" alt="Matthias"/><br /><sub><b>Matthias</b></sub></a><br /><a href="https://github.com/codemod-com/codemod/issues?q=author%3Axmatthias" title="Bug reports">🐛</a></td>
    </tr>
    <tr>
      <td align="center" valign="top" width="14.28%"><a href="http://minimalistweb.dev"><img src="https://avatars.githubusercontent.com/u/37929992?v=4?s=100" width="100px;" alt="Mohammad Bagher Abiyat"/><br /><sub><b>Mohammad Bagher Abiyat</b></sub></a><br /><a href="#infra-Aslemammad" title="Infrastructure (Hosting, Build-Tools, etc)">🚇</a></td>
      <td align="center" valign="top" width="14.28%"><a href="https://github.com/Yugal41735"><img src="https://avatars.githubusercontent.com/u/74638775?v=4?s=100" width="100px;" alt="Yugal Agarwal"/><br /><sub><b>Yugal Agarwal</b></sub></a><br /><a href="https://github.com/codemod-com/codemod/commits?author=Yugal41735" title="Code">💻</a></td>
      <td align="center" valign="top" width="14.28%"><a href="https://github.com/poswalsameer"><img src="https://avatars.githubusercontent.com/u/106386145?v=4?s=100" width="100px;" alt="Sameer Poswal"/><br /><sub><b>Sameer Poswal</b></sub></a><br /><a href="https://github.com/codemod-com/codemod/commits?author=poswalsameer" title="Code">💻</a> <a href="https://github.com/codemod-com/codemod/issues?q=author%3Aposwalsameer" title="Bug reports">🐛</a></td>
      <td align="center" valign="top" width="14.28%"><a href="https://github.com/stramel"><img src="https://avatars.githubusercontent.com/u/855184?v=4?s=100" width="100px;" alt="Michael Stramel"/><br /><sub><b>Michael Stramel</b></sub></a><br /><a href="https://github.com/codemod-com/codemod/commits?author=stramel" title="Documentation">📖</a></td>
      <td align="center" valign="top" width="14.28%"><a href="https://georgewl.dev"><img src="https://avatars.githubusercontent.com/u/5931248?v=4?s=100" width="100px;" alt="George W Langham"/><br /><sub><b>George W Langham</b></sub></a><br /><a href="https://github.com/codemod-com/codemod/issues?q=author%3AGeorgeWL" title="Bug reports">🐛</a></td>
      <td align="center" valign="top" width="14.28%"><a href="https://github.com/sid-maddy"><img src="https://avatars.githubusercontent.com/u/10049286?v=4?s=100" width="100px;" alt="Siddhesh Mhadnak"/><br /><sub><b>Siddhesh Mhadnak</b></sub></a><br /><a href="https://github.com/codemod-com/codemod/commits?author=sid-maddy" title="Documentation">📖</a></td>
    </tr>
  </tbody>
</table>

<!-- markdownlint-restore -->
<!-- prettier-ignore-end -->

<!-- ALL-CONTRIBUTORS-LIST:END -->

This project follows the [all-contributors](https://github.com/all-contributors/all-contributors) specification. Contributions of any kind welcome!
