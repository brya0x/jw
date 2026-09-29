package commands

import (
	"flag"
	"strings"
)

func (a *App) runInfo(args []string) error {
	name, args := splitName(args)
	fs := flag.NewFlagSet("info", flag.ExitOnError)
	asJSON := fs.Bool("json", false, "print as JSON (for scripts and agents)")
	fs.Parse(args)

	p, e, err := open(name)
	if err != nil {
		return err
	}
	s := a.streamJSON(a.row(p, *e), p.cfg)
	if *asJSON {
		return a.writeJSON(s)
	}

	a.printf("%s  %s\n", s.Name, s.ID)
	a.printf("  project  %s   (config %s)\n", s.Project, configName(p.cfg))
	branch := s.Branch
	if s.Adopted {
		branch += "  (adopted: existed before jw)"
	}
	a.printf("  branch   %s\n", branch)
	path := s.Path
	if s.Missing {
		path += "  (missing)"
	}
	a.printf("  path     %s\n", path)
	a.printf("  slot     %d   (ports %d–%d)\n", s.Slot, s.PortBase, s.PortBase+99)

	width := 0
	for svc := range s.Ports {
		width = max(width, len(svc))
	}
	for i, svc := range sortedKeys(s.Ports) {
		label := "           "
		if i == 0 {
			label = "  ports    "
		}
		up := ""
		if s.Ports[svc].Listening {
			up = "  listening"
		}
		a.printf("%s%-*s  %d%s\n", label, width, svc, s.Ports[svc].Port, up)
	}

	tab := "closed"
	if s.Open {
		tab = "open in " + s.Tab
	}
	a.printf("  tab      %s\n", tab)
	switch {
	case s.PR != nil:
		a.printf("  pr       #%d %s  %s\n", s.PR.Number, s.PR.State, s.PR.URL)
	case s.PRUnknown:
		a.printf("  pr       ? (gh unavailable)\n")
	default:
		a.printf("  pr       none\n")
	}
	if s.State != "" {
		a.printf("  state    %s\n", s.State)
	}
	if svcs := sortedKeys(p.cfg.Dev); len(svcs) > 0 {
		a.printf("  dev      %s   (jw dev <service>)\n", strings.Join(svcs, ", "))
	}
	return nil
}
