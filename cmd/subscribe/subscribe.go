package subscribe

import (
	"github.com/spf13/cobra"
)

// readCmd represents the read command
var SubscribeCmd = &cobra.Command{
	Use:   "subscribe",
	Short: "Subscribe to the streams in the database.",
	Long: `Subscribe to the database using either Persistent Subscriptions
	or Catch-up Subscriptions.
	
	Two modes are available:
	1. Catchup
	2. Persistent`,
	Run: func(cmd *cobra.Command, args []string) {
		cmd.Help()
	},
}

func init() {
	// rootCmd.AddCommand(readCmd)

	SubscribeCmd.AddCommand(catchupCmd)
	SubscribeCmd.AddCommand(persistentCmd)

	// Here you will define your flags and configuration settings.

	// Cobra supports Persistent Flags which will work for this command
	// and all subcommands, e.g.:
	// readCmd.PersistentFlags().String("foo", "", "A help for foo")

	// Cobra supports local flags which will only run when this command
	// is called directly, e.g.:
	// readCmd.Flags().BoolP("toggle", "t", false, "Help message for toggle")
}
