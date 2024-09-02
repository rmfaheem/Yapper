package subscribe

import (
	"fmt"

	"github.com/spf13/cobra"
)

// readCmd represents the read command
var catchupCmd = &cobra.Command{
	Use:   "catchup",
	Short: "Catchup subscriptions",
	Long: `Subscribe to the database with Catch-up subscriptions.
	All subscriptions will be automatically killed after all events have been processed
	or after the event count is passed.
	
	Example:
	$ `,
	Run: func(cmd *cobra.Command, args []string) {
		fmt.Println("catchup subscription called")
	},
}

func init() {
	// rootCmd.AddCommand(readCmd)

	// Here you will define your flags and configuration settings.

	// Cobra supports Persistent Flags which will work for this command
	// and all subcommands, e.g.:
	// readCmd.PersistentFlags().String("foo", "", "A help for foo")

	// Cobra supports local flags which will only run when this command
	// is called directly, e.g.:
	// readCmd.Flags().BoolP("toggle", "t", false, "Help message for toggle")
}
