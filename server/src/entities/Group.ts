import { Entity, JoinTable, OneToMany, PrimaryColumn } from "typeorm";
import { User } from "./User";
import { File } from "./File";

@Entity()
export class Group {
  @PrimaryColumn()
  gid: number;

  @OneToMany(() => User, (user) => user.group)
  @JoinTable()
  users: User[];

  @OneToMany(() => File, (file) => file.group)
  files: File[];
}