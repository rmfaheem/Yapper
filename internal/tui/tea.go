package tui

import (
	"fmt"
	"os"
	"strings"

	"github.com/charmbracelet/bubbles/cursor"
	"github.com/charmbracelet/bubbles/textinput"
	"github.com/charmbracelet/bubbles/viewport"
	tea "github.com/charmbracelet/bubbletea"
	"github.com/charmbracelet/lipgloss"
)

const (
	darkGray  = lipgloss.Color("#767676")
	blue      = lipgloss.Color("#09254a")
	green     = lipgloss.Color("#5bb553")
	lightBlue = lipgloss.Color("#34aaeb")
)

var (
	contentStyle = func() lipgloss.Style {
		b := lipgloss.NormalBorder()
		b.Right = "├"
		return lipgloss.NewStyle().
			BorderStyle(b).PaddingLeft(1).PaddingRight(1).BorderForeground(green)
	}()

	inputStyle = func() lipgloss.Style {
		return lipgloss.NewStyle().Foreground(darkGray)
	}()
)

// type field struct {
// 	name, description, min, max, defolt string
// }

// type command struct {
// 	title, desc string
// 	fields      []field
// }

// var commands = []command{
// 	{"help", "Show all valid commands", []field{}},
// 	{"config", "View/edit yapper config", []field{}},
// 	{"wrfl", "Write flood", []field{
// 		{"Clients", "Number of concurrent clients", "1", "65535", "1"},
// 		{"Requests", "Number of concurrent requests per client", "1", "65535", "1"},
// 		{"Stream Count", "Total number of streams to create", "1", "2147483647", "1"},
// 		{"Event Size", "Average event size in bytes", "10", "16777216", "10"},
// 		{"Batch Size", "Batch size per write", "1", "65535", "1"},
// 		{"Stream Prefix", "Prefix for the generated streams", "", "", ""},
// 	}},
// 	{"rdfl", "Read flood", []field{
// 		{"Clients", "Number of concurrent clients", "1", "65535", "1"},
// 		{"Requests", "Number of concurrent requests per client", "1", "65535", "1"},
// 		{"Stream Count", "Number of streams to read", "1", "2147483647", "1"},
// 		{"Batch Size", "Batch size per read", "1", "65535", "1"},
// 		{"Stream Prefix", "Prefix for the streams we want to read", "", "", ""},
// 	}},
// }

type model struct {
	showhelp   bool
	config     bool
	mvpContent []string
	mvp        viewport.Model
	svpContent []string
	svp        viewport.Model
	focused    int
	input      textinput.Model
	err        error
}

func RenderUI() {
	p := tea.NewProgram(initialModel())
	if _, err := p.Run(); err != nil {
		fmt.Printf("Alas, there's been an error: %v", err)
		os.Exit(1)
	}
}

func initialModel() model {
	mvp := viewport.New(30, 20)
	mvp.SetContent(lipgloss.NewStyle().Foreground(green).Render(`All output will be shown here.`) + helpText())
	mvp.Style = (lipgloss.NewStyle().Border(lipgloss.NormalBorder(), false, true, true, true)).Padding(0, 1).BorderForeground(lightBlue)

	ti := textinput.New()
	ti.Placeholder = "help"
	// ti.Prompt = "🗣  "
	ti.Focus()

	ti.CharLimit = 256
	ti.Width = 20

	svp := viewport.New(30, 20)
	svp.SetContent(helpText())
	svp.Style = mvp.Style
	svp.Height = mvp.Height
	svp.Width = mvp.Width

	return model{
		mvpContent: []string{},
		mvp:        mvp,
		svpContent: []string{},
		svp:        svp,
		input:      ti,
		err:        nil,
	}
}

func helpText() string {
	return `

Available commands:

	help: Show available commands

	configure: Configure yapper
		Params:
			--gossip-seed: gossip seed (e.g. 127.0.0.1:2113)
			--username: username (e.g.admin)
			--password: password (e.g.changeit)
			--use-tls: whether or not the connection should be encrypted (y/n)
			--verify-cert: verify certificate (y/n)
			--root-ca-path: path to root certificate (optional)
			--node-preference: node preference (leader/follower/random) 

	wrfl: Write flood
		Params:
			--clients: number of concurrent clients
			--requests: number of concurrent requests per client
			--streams: total number of streams to create
			--event-size: average size of each event in bytes (max 16777216 i.e 16MB)
			--batch-size: batch size for each write operation
			--stream-prefix: prefix for streams created by this command
	
	rdfl: Read flood
		Params:
			--clients: number of concurrent clients
			--requests: number of concurrent requests per client
			--streams: total number of streams to create
			--batch-size: batch size for each write operation
			--stream-prefix: prefix for streams created by this command

	Ctrl + D to go back
`
}

func (m model) Init() tea.Cmd {
	return textinput.Blink
}

func (m model) Update(msg tea.Msg) (tea.Model, tea.Cmd) {
	switch msg := msg.(type) {

	case tea.WindowSizeMsg:
		if m.config || m.showhelp {
			m.svp.Height = msg.Height - lipgloss.Height(m.input.View()) - lipgloss.Height(m.headerView())
			m.svp.Width = msg.Width
			m.input.Width = msg.Width
		} else {
			m.mvp.Height = msg.Height - lipgloss.Height(m.input.View()) - lipgloss.Height(m.headerView())
			m.mvp.Width = msg.Width
			m.input.Width = msg.Width
		}
		return m, nil

	// Is it a key press?
	case tea.KeyMsg:

		// Cool, what was the actual key pressed?
		switch msg.String() {

		// These keys should exit the program.
		case "ctrl+c", "esc", "ctrl+d":
			if m.showhelp {
				m.showhelp = false
				m.input.Placeholder = "help"
				return m, nil
			}
			return m, tea.Quit

		// The "up" and "k" keys move the cursor up
		case "tab":
			return m, nil

		// The "enter" key and the spacebar (a literal space) toggle
		// the selected state for the item that the cursor is pointing at.
		case "enter":
			v := strings.TrimSpace(m.input.Value())

			if v == "" {
				// Don't send empty messages.
				return m, nil
			}

			// Simulate sending a message. In your application you'll want to
			// also return a custom command to send the message off to
			// a server.
			switch v {
			case "help":
				m.showhelp = true
				m.svp.Height = m.mvp.Height
				m.svp.Width = m.mvp.Width
				m.input.Placeholder = "Esc / Ctrl + D to go close."
			default:
				if strings.HasPrefix(v, "wrfl") {
					ProcessCommand(v)
				} else if strings.HasPrefix(v, "rdfl") {
					ProcessCommand(v)
				} else {
					m.mvp.SetContent(fmt.Sprintf("Unknown command: %s", v))
				}
			}
			// m.content = append(m.content, v)
			// m.viewport.SetContent(strings.Join(m.content, "\n"))
			m.input.Reset()
			m.mvp.GotoBottom()
			return m, nil
		case "clear":
			m.mvp.SetContent("")
			m.input.Reset()
			m.mvp.GotoBottom()
			return m, nil

		default:
			// Send all other keypresses to the textarea.
			var cmd tea.Cmd
			m.input, cmd = m.input.Update(msg)
			return m, cmd
		}

	case cursor.BlinkMsg:
		var cmd tea.Cmd
		m.input, cmd = m.input.Update(msg)
		return m, cmd

	default:
		return m, nil
	}
}

func (m model) View() string {
	if m.config || m.showhelp {
		return fmt.Sprintf("%s\n%s\n%s", m.headerView(), m.svp.View(), m.input.View())
	}
	return fmt.Sprintf("%s\n%s\n%s", m.headerView(), m.mvp.View(), m.input.View())
}

func (m model) headerView() string {
	info := contentStyle.Foreground(green).BorderForeground(lightBlue).Render("Yapper 🗣 ")
	line := strings.Repeat("─", max(0, m.mvp.Width-lipgloss.Width(info))-1) + "┐"
	return lipgloss.JoinHorizontal(lipgloss.Center, info, lipgloss.NewStyle().Foreground(lightBlue).Render(line))
}

func (m model) processCommand(cmd string) {
	// args := strings.Split(cmd, " ")
	// newArgs := [5]int{}
	// for i, arg := range args {

	// 	newArgs[i], _ = strconv.Atoi(arg)
	// }
	// if strings.TrimSpace(args[0]) == "wrfl" {
	// 	m.wrfl(newArgs[1:])
	// } else if strings.TrimSpace(args[0]) == "rdfl" {
	// 	m.rdfl(newArgs[1:])
	// }
}

func (m model) wrfl(args []string) {
	// outputChannel := make(chan string)
	// ops.Wrfl(&outputChannel, strconv.Atoi(args[0]), args[1], args[2], args[3], args[4])

}

func (m model) rdfl(args []string) {

}
