import {
    Cloud,
    HardDrive,
    Settings,
    Activity,
    Plus,
    ArrowUpRight,
} from "lucide-react";

const plannedProviders = ["S3", "SFTP", "WebDAV"];

export function App() {
    return (
        <main className="app-shell">
            <aside className="sidebar">
                <div className="brand-lockup">
                    <div className="brand-mark" aria-hidden="true">
                        <Cloud size={18} strokeWidth={2.5} />
                    </div>
                    <div>
                        <strong>Bifrost Drive</strong>
                        <span>One gateway. Every storage.</span>
                    </div>
                </div>
                <nav aria-label="Primary navigation">
                    <a className="nav-item active" href="#connections">
                        <HardDrive size={17} /> Connections
                    </a>
                    <a className="nav-item" href="#activity">
                        <Activity size={17} /> Activity
                    </a>
                    <a className="nav-item" href="#settings">
                        <Settings size={17} /> Settings
                    </a>
                </nav>
                <div className="sidebar-footer">
                    <span className="status-dot" /> Service ready
                    <small>Foundation build 0.1.0</small>
                </div>
            </aside>
            <section className="content" id="connections">
                <header className="topbar">
                    <div>
                        <p className="eyebrow">Storage workspace</p>
                        <h1>Your connections</h1>
                        <p className="lede">
                            Remote files, ready when you are.
                        </p>
                    </div>
                    <button className="primary-button" type="button">
                        <Plus size={17} /> Add connection
                    </button>
                </header>
                <section
                    className="welcome-panel"
                    aria-labelledby="welcome-title"
                >
                    <div>
                        <span className="panel-kicker">
                            Early access foundation
                        </span>
                        <h2 id="welcome-title">
                            A quieter way to reach every file.
                        </h2>
                        <p>
                            Bifrost Drive is establishing the secure bridge
                            between your desktop and remote storage. The first
                            provider workflow is being built now.
                        </p>
                    </div>
                    <div className="bridge-graphic" aria-hidden="true">
                        <span />
                        <span />
                        <span />
                    </div>
                </section>
                <section className="section-heading">
                    <div>
                        <p className="eyebrow">Available soon</p>
                        <h2>Connection types</h2>
                    </div>
                    <span className="muted-label">
                        {plannedProviders.length} providers planned
                    </span>
                </section>
                <div className="provider-grid">
                    {plannedProviders.map((provider) => (
                        <article className="provider-card" key={provider}>
                            <div className="provider-icon">
                                <Cloud size={20} />
                            </div>
                            <div>
                                <h3>{provider}</h3>
                                <p>Provider adapter in progress</p>
                            </div>
                            <ArrowUpRight className="card-arrow" size={17} />
                        </article>
                    ))}
                </div>
            </section>
        </main>
    );
}
