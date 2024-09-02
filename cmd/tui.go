package cmd

import (
	"github.com/rmfaheem/yapper/internal/tui"
	"github.com/spf13/cobra"
)

// tuiCmd represents the tui command
var tuiCmd = &cobra.Command{
	Use:   "tui",
	Short: "Launch the yapper with the TUI",
	Long: `The yapper TUI provides a beautiful interface which 
	can be used to perform everything that is possible via commandline.`,
	Run: func(cmd *cobra.Command, args []string) {
		tui.RenderUI()
	},
}

func init() {
	// rootCmd.AddCommand(tuiCmd)

	// Here you will define your flags and configuration settings.

	// Cobra supports Persistent Flags which will work for this command
	// and all subcommands, e.g.:
	// tuiCmd.PersistentFlags().String("foo", "", "A help for foo")

	// Cobra supports local flags which will only run when this command
	// is called directly, e.g.:
	// tuiCmd.Flags().BoolP("toggle", "t", false, "Help message for toggle")
}
