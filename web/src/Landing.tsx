import { useEffect } from 'react'
import { Link } from 'react-router-dom'
import { ArrowDown, ArrowUpRight } from 'lucide-react'
import { CopyButton } from './ui'
import landscape from './assets/kiln-landscape.webp'
import './landing.css'

const source = 'https://github.com/Jabbslad/kiln'
const install =
  'curl -fsSL https://raw.githubusercontent.com/Jabbslad/kiln/main/deploy/bootstrap/install.sh | sh'

export default function Landing() {
  useEffect(() => {
    document.title = 'kiln — Linux boxes on your terms'
  }, [])
  return (
    <div className="landing">
      <a className="skip-link" href="#main">
        Skip to content
      </a>
      <header className="landing-header">
        <Link className="landing-brand" to="/" aria-label="kiln home">
          kiln.
        </Link>
        <nav aria-label="Site navigation">
          <a href="#platform">Platform</a>
          <a href="#install">Install</a>
          <a href={source}>
            Source <ArrowUpRight size={12} />
          </a>
        </nav>
        <Link className="button" to="/boxes">
          Open console <ArrowUpRight size={14} />
        </Link>
      </header>
      <main id="main" tabIndex={-1}>
        <section className="landing-hero" aria-labelledby="hero-title">
          <img src={landscape} width="1738" height="905" alt="" fetchPriority="high" />
          <div className="hero-content">
            <p className="eyebrow">Self-hosted infrastructure / open source</p>
            <h1 id="hero-title">
              Your machine.
              <br />
              More possibilities.
            </h1>
            <p className="hero-description">
              Persistent Linux boxes, on hardware you control.
              <br className="desktop-break" /> Built with Rust. Powered by Firecracker.
            </p>
            <div className="hero-actions">
              <a className="button primary" href="#install">
                Install Kiln <ArrowDown size={14} />
              </a>
              <a className="button" href={`${source}/blob/main/docs/runtime.md`}>
                Explore the runtime <ArrowUpRight size={14} />
              </a>
            </div>
            <p className="hero-caption">
              <span className="terminal-prompt">$</span> kiln create --name my-workspace
            </p>
          </div>
          <span className="hero-footnote">Your host. Your disks. Your workflow.</span>
        </section>
        <div className="platform-band" aria-label="Platform technologies">
          <span>Rust control plane</span>
          <span>Firecracker microVMs</span>
          <span>Linux / KVM</span>
          <span>SSH + SFTP</span>
        </div>
        <section className="landing-section" id="platform" aria-labelledby="platform-title">
          <header className="landing-section-heading">
            <p className="eyebrow">One host. Separate workspaces.</p>
            <h2 id="platform-title">A small box for the next big idea.</h2>
            <p>
              A development environment you can return to. Prepare a template, create a box, and
              connect from your terminal.
            </p>
          </header>
          <div className="workflow-grid">
            <article>
              <div className="workflow-code">
                <span className="terminal-prompt">$</span> kiln templates
              </div>
              <h3>
                <span>01</span> Prepare
              </h3>
              <p>
                Start from a prepared guest template. Snapshot-based creation avoids installing an
                operating system for every box.
              </p>
            </article>
            <article>
              <div className="workflow-code">
                <span className="terminal-prompt">$</span> kiln create --name api-dev
              </div>
              <h3>
                <span>02</span> Create
              </h3>
              <p>
                Give each project its own Linux environment. Keep its files on disk between
                sessions, with explicit CPU and memory allocations.
              </p>
            </article>
            <article>
              <div className="workflow-code">
                <span className="terminal-prompt">$</span> kiln ssh api-dev
              </div>
              <h3>
                <span>03</span> Connect
              </h3>
              <p>
                Use a real shell, run commands, or transfer files with SFTP. Your client connects
                directly to your enrolled server.
              </p>
            </article>
          </div>
        </section>
        <section className="console-feature" aria-labelledby="console-title">
          <div>
            <p className="eyebrow">The console / design preview</p>
            <h2 id="console-title">Keep the whole host in view.</h2>
            <p>
              Explore box inventory, templates, and operation details in a read-only design preview.
              Browser login and live controls are not integrated yet.
            </p>
            <Link className="button" to="/boxes?preview=1">
              Explore design preview <ArrowUpRight size={14} />
            </Link>
          </div>
          <div
            className="console-figure"
            aria-label="Illustration of a host and separate box disks"
          >
            <div className="figure-label">Architecture / simplified</div>
            <div className="host-figure">
              <span className="eyebrow">Your Linux host</span>
              <div className="figure-boxes">
                <div>
                  <span>Project A</span>
                  <small>microVM</small>
                  <span className="figure-disk">Persistent disk</span>
                </div>
                <div>
                  <span>Project B</span>
                  <small>microVM</small>
                  <span className="figure-disk">Persistent disk</span>
                </div>
              </div>
              <div className="figure-control">
                Kiln control plane <span>Rust / Firecracker</span>
              </div>
            </div>
            <p>No live host data is shown.</p>
          </div>
        </section>
        <section
          className="landing-section install-section"
          id="install"
          aria-labelledby="install-title"
        >
          <header className="landing-section-heading">
            <p className="eyebrow">Start on your own hardware</p>
            <h2 id="install-title">A terminal is all you need.</h2>
            <p>Install the client on your laptop. Set up and enroll a server before signing in.</p>
          </header>
          <div className="install-command">
            <code>
              <span className="terminal-prompt" aria-hidden="true">
                $
              </span>
              {install}
            </code>
            <CopyButton value={install} label="Copy client install command" />
          </div>
          <div className="install-details">
            <div>
              <h3>On your laptop</h3>
              <p>
                macOS Apple Silicon or x86-64 Linux with glibc 2.39+. No GitHub token required to
                download.
              </p>
              <a className="text-action" href={`${source}/releases`}>
                Other client downloads <ArrowUpRight size={13} />
              </a>
            </div>
            <div>
              <h3>On your server</h3>
              <p>
                A supported x86-64 Linux host with KVM and systemd. The server installer changes
                system packages, accounts, and services.
              </p>
              <a className="text-action" href={`${source}/blob/main/deploy/bootstrap/README.md`}>
                Read the setup guide <ArrowUpRight size={13} />
              </a>
            </div>
            <div>
              <h3>Then sign in</h3>
              <p>
                Run <code>kiln login</code> with the GitHub or Google account that enrolled your
                server. Private hosts still require LAN or VPN access.
              </p>
              <a className="text-action" href={`${source}/blob/main/docs/remote-client.md`}>
                Client reference <ArrowUpRight size={13} />
              </a>
            </div>
          </div>
          <p className="pilot-note">
            Kiln is a trusted-workload pilot, not a production multi-tenant sandbox. No hosted
            compute is provisioned by signing in.
          </p>
        </section>
      </main>
      <footer className="landing-footer">
        <span className="landing-brand">kiln.</span>
        <span>Build locally. Keep control.</span>
        <a href={source}>
          Source on GitHub <ArrowUpRight size={13} />
        </a>
      </footer>
    </div>
  )
}
